use crate::error::NdkError;
use crate::manifest::AndroidManifest;
use crate::ndk::{Key, Ndk};
use crate::target::Target;
use std::collections::HashMap;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The options for how to treat debug symbols that are present in any `.so`
/// files that are added to the APK.
///
/// Using [`strip`](https://doc.rust-lang.org/cargo/reference/profiles.html#strip)
/// or [`split-debuginfo`](https://doc.rust-lang.org/cargo/reference/profiles.html#split-debuginfo)
/// in your cargo manifest(s) may cause debug symbols to not be present in a
/// `.so`, which would cause these options to do nothing.
#[derive(Debug, Copy, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StripConfig {
    /// Does not treat debug symbols specially
    Default,
    /// Removes debug symbols from the library before copying it into the APK
    Strip,
    /// Splits the library into into an ELF (`.so`) and DWARF (`.dwarf`). Only the
    /// `.so` is copied into the APK
    Split,
}

impl Default for StripConfig {
    fn default() -> Self {
        Self::Default
    }
}

pub struct ApkConfig {
    pub ndk: Ndk,
    pub build_dir: PathBuf,
    pub apk_name: String,
    pub assets: Option<PathBuf>,
    pub resources: Option<PathBuf>,
    pub manifest: AndroidManifest,
    pub disable_aapt_compression: bool,
    pub strip: StripConfig,
    pub reverse_port_forward: HashMap<String, String>,
}

impl ApkConfig {
    fn build_tool(&self, tool: &'static str) -> Result<Command, NdkError> {
        let mut cmd = self.ndk.build_tool(tool)?;
        cmd.current_dir(&self.build_dir);
        Ok(cmd)
    }

    fn unaligned_apk(&self) -> PathBuf {
        self.build_dir
            .join(format!("{}-unaligned.apk", self.apk_name))
    }

    /// Retrieves the path of the APK that will be written when [`UnsignedApk::sign`]
    /// is invoked
    #[inline]
    pub fn apk(&self) -> PathBuf {
        self.build_dir.join(format!("{}.apk", self.apk_name))
    }

    pub fn create_apk(&self) -> Result<UnalignedApk, NdkError> {
        std::fs::create_dir_all(&self.build_dir)?;
        self.manifest.write_to(&self.build_dir)?;

        let target_sdk_version = self
            .manifest
            .sdk
            .target_sdk_version
            .unwrap_or_else(|| self.ndk.default_target_platform());
        let mut aapt = self.build_tool(bin!("aapt"))?;
        aapt.arg("package")
            .arg("-f")
            .arg("-F")
            .arg(self.unaligned_apk())
            .arg("-M")
            .arg("AndroidManifest.xml")
            .arg("-I")
            .arg(self.ndk.android_jar(target_sdk_version)?);

        if self.disable_aapt_compression {
            aapt.arg("-0").arg("");
        }

        if let Some(res) = &self.resources {
            aapt.arg("-S").arg(res);
        }

        if let Some(assets) = &self.assets {
            aapt.arg("-A").arg(assets);
        }

        if !aapt.status()?.success() {
            return Err(NdkError::CmdFailed(aapt));
        }

        Ok(UnalignedApk {
            config: self,
            pending_libs: HashSet::default(),
        })
    }
}

pub struct UnalignedApk<'a> {
    config: &'a ApkConfig,
    pending_libs: HashSet<String>,
}

impl<'a> UnalignedApk<'a> {
    pub fn config(&self) -> &ApkConfig {
        self.config
    }

    pub fn add_lib(&mut self, path: &Path, target: Target) -> Result<(), NdkError> {
        if !path.exists() {
            return Err(NdkError::PathNotFound(path.into()));
        }
        let abi = target.android_abi();
        let lib_path = Path::new("lib").join(abi).join(path.file_name().unwrap());
        let out = self.config.build_dir.join(&lib_path);
        std::fs::create_dir_all(out.parent().unwrap())?;

        match self.config.strip {
            StripConfig::Default => {
                std::fs::copy(path, out)?;
            }
            StripConfig::Strip | StripConfig::Split => {
                let obj_copy = self.config.ndk.toolchain_bin("objcopy", target)?;

                {
                    let mut cmd = Command::new(&obj_copy);
                    cmd.arg("--strip-debug");
                    cmd.arg(path);
                    cmd.arg(&out);

                    if !cmd.status()?.success() {
                        return Err(NdkError::CmdFailed(cmd));
                    }
                }

                if self.config.strip == StripConfig::Split {
                    let dwarf_path = out.with_extension("dwarf");

                    {
                        let mut cmd = Command::new(&obj_copy);
                        cmd.arg("--only-keep-debug");
                        cmd.arg(path);
                        cmd.arg(&dwarf_path);

                        if !cmd.status()?.success() {
                            return Err(NdkError::CmdFailed(cmd));
                        }
                    }

                    let mut cmd = Command::new(obj_copy);
                    cmd.arg(format!("--add-gnu-debuglink={}", dwarf_path.display()));
                    cmd.arg(out);

                    if !cmd.status()?.success() {
                        return Err(NdkError::CmdFailed(cmd));
                    }
                }
            }
        }

        // Pass UNIX path separators to `aapt` on non-UNIX systems, ensuring the resulting separator
        // is compatible with the target device instead of the host platform.
        // Otherwise, it results in a runtime error when loading the NativeActivity `.so` library.
        let lib_path_unix = lib_path.to_str().unwrap().replace('\\', "/");

        self.pending_libs.insert(lib_path_unix);

        Ok(())
    }

    pub fn add_runtime_libs(
        &mut self,
        path: &Path,
        target: Target,
        search_paths: &[&Path],
    ) -> Result<(), NdkError> {
        let abi_dir = path.join(target.android_abi());
        for entry in fs::read_dir(&abi_dir).map_err(|e| NdkError::IoPathError(abi_dir, e))? {
            let entry = entry?;
            let path = entry.path();
            if path.extension() == Some(OsStr::new("so")) {
                self.add_lib_recursively(&path, target, search_paths)?;
            }
        }
        Ok(())
    }

    pub fn add_pending_libs_and_align(self) -> Result<UnsignedApk<'a>, NdkError> {
        let mut aapt = self.config.build_tool(bin!("aapt"))?;
        aapt.arg("add");

        if self.config.disable_aapt_compression {
            aapt.arg("-0").arg("");
        }

        aapt.arg(self.config.unaligned_apk());

        for lib_path_unix in self.pending_libs {
            aapt.arg(lib_path_unix);
        }

        if !aapt.status()?.success() {
            return Err(NdkError::CmdFailed(aapt));
        }

        let mut zipalign = self.config.build_tool(bin!("zipalign"))?;
        zipalign
            .arg("-f")
            .arg("-v")
            .arg("4")
            .arg(self.config.unaligned_apk())
            .arg(self.config.apk());

        if !zipalign.status()?.success() {
            return Err(NdkError::CmdFailed(zipalign));
        }

        Ok(UnsignedApk(self.config))
    }
}

pub struct UnsignedApk<'a>(&'a ApkConfig);

impl<'a> UnsignedApk<'a> {
    pub fn sign(self, key: Key) -> Result<Apk, NdkError> {
        let mut apksigner = self.0.build_tool(bat!("apksigner"))?;
        apksigner
            .arg("sign")
            .arg("--ks")
            .arg(&key.path)
            .arg("--ks-pass")
            .arg(format!("pass:{}", &key.password))
            .arg(self.0.apk());
        if !apksigner.status()?.success() {
            return Err(NdkError::CmdFailed(apksigner));
        }
        Ok(Apk::from_config(self.0))
    }
}

pub struct Apk {
    path: PathBuf,
    package_name: String,
    ndk: Ndk,
    reverse_port_forward: HashMap<String, String>,
}

impl Apk {
    pub fn from_config(config: &ApkConfig) -> Self {
        let ndk = config.ndk.clone();
        Self {
            path: config.apk(),
            package_name: config.manifest.package.clone(),
            ndk,
            reverse_port_forward: config.reverse_port_forward.clone(),
        }
    }

    pub fn reverse_port_forwarding(&self, device_serial: Option<&str>) -> Result<(), NdkError> {
        for (from, to) in &self.reverse_port_forward {
            println!("Reverse port forwarding from {} to {}", from, to);
            let mut adb = self.ndk.adb(device_serial)?;

            adb.arg("reverse").arg(from).arg(to);

            if !adb.status()?.success() {
                return Err(NdkError::CmdFailed(adb));
            }
        }

        Ok(())
    }

    pub fn install(&self, device_serial: Option<&str>) -> Result<(), NdkError> {
        let mut adb = self.ndk.adb(device_serial)?;

        adb.arg("install").arg("-r").arg(&self.path);
        if !adb.status()?.success() {
            return Err(NdkError::CmdFailed(adb));
        }
        Ok(())
    }

    pub fn start(&self, device_serial: Option<&str>) -> Result<(), NdkError> {
        let mut adb = self.ndk.adb(device_serial)?;
        adb.arg("shell")
            .arg("am")
            .arg("start")
            .arg("-a")
            .arg("android.intent.action.MAIN")
            .arg("-n")
            .arg(format!("{}/android.app.NativeActivity", self.package_name));

        if !adb.status()?.success() {
            return Err(NdkError::CmdFailed(adb));
        }

        Ok(())
    }

    pub fn uidof(&self, device_serial: Option<&str>) -> Result<u32, NdkError> {
        let mut adb = self.ndk.adb(device_serial)?;
        adb.arg("shell")
            .arg("pm")
            .arg("list")
            .arg("package")
            .arg("-U")
            .arg(&self.package_name);
        let output = adb.output()?;

        if !output.status.success() {
            return Err(NdkError::CmdFailed(adb));
        }

        let output = std::str::from_utf8(&output.stdout).unwrap();
        let (_package, uid) = output
            .lines()
            .filter_map(|line| line.split_once(' '))
            // `pm list package` uses the id as a substring filter; make sure
            // we select the right package in case it returns multiple matches:
            .find(|(package, _uid)| package.strip_prefix("package:") == Some(&self.package_name))
            .ok_or(NdkError::PackageNotInOutput {
                package: self.package_name.clone(),
                output: output.to_owned(),
            })?;
        let uid = uid
            .strip_prefix("uid:")
            .ok_or(NdkError::UidNotInOutput(output.to_owned()))?;
        uid.parse()
            .map_err(|e| NdkError::NotAUid(e, uid.to_owned()))
    }
}
