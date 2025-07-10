use crate::error::NdkError;
use serde::{Deserialize, Serialize, Serializer};
use std::{
    borrow::Cow,
    fs::File,
    path::{Path, PathBuf},
};

/// Represents either an [`AndroidManifest`] parsed from the Cargo manifest file, or the path to the specified Android manifest file.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum AndroidManifestInput {
    FromToml(AndroidManifest),
    FromXml(PathBuf),
}

impl AndroidManifestInput {
    pub fn is_xml(&self) -> bool {
        matches!(self, AndroidManifestInput::FromXml(_))
    }

    /// Returns the `AndroidManifest` information, either cloned or parsed.
    /// If it is parsed, some items in the XML may be missing in the parsed information.
    pub fn get_manifest_info(&self) -> Result<Cow<AndroidManifest>, NdkError> {
        Ok(match &self {
            AndroidManifestInput::FromToml(manifest) => Cow::Borrowed(manifest),
            AndroidManifestInput::FromXml(file) => Cow::Owned(AndroidManifest::read_from(file)?),
        })
    }

    pub fn write_to(&self, dir: &Path) -> Result<(), NdkError> {
        match &self {
            AndroidManifestInput::FromToml(manifest) => manifest.write_to(dir),
            AndroidManifestInput::FromXml(file) => {
                Ok(std::fs::copy(file, dir.join("AndroidManifest.xml")).map(|_| ())?)
            }
        }
    }
}

// Note: these serde `rename`s are XML names for XML (de)serialization; `alias`es are for TOML deserialization.

/// Android [manifest element](https://developer.android.com/guide/topics/manifest/manifest-element), containing an [`Application`] element.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename = "manifest")]
pub struct AndroidManifest {
    #[serde(rename = "xmlns:android", alias = "ns_android")]
    #[serde(default = "default_namespace")]
    ns_android: String,
    #[serde(default)]
    pub package: String,
    #[serde(rename = "android:sharedUserId", alias = "shared_user_id")]
    pub shared_user_id: Option<String>,
    #[serde(rename = "android:versionCode", alias = "version_code")]
    pub version_code: Option<u32>,
    #[serde(rename = "android:versionName", alias = "version_name")]
    pub version_name: Option<String>,

    #[serde(rename = "uses-sdk", alias = "sdk")]
    #[serde(default)]
    pub sdk: Sdk,

    #[serde(rename = "uses-feature", alias = "uses_feature")]
    #[serde(default)]
    pub uses_feature: Vec<Feature>,
    #[serde(rename = "uses-permission", alias = "uses_permission")]
    #[serde(default)]
    pub uses_permission: Vec<Permission>,

    #[serde(default)]
    pub queries: Option<Queries>,

    #[serde(default)]
    pub application: Application,
}

impl Default for AndroidManifest {
    fn default() -> Self {
        Self {
            ns_android: default_namespace(),
            package: Default::default(),
            shared_user_id: Default::default(),
            version_code: Default::default(),
            version_name: Default::default(),
            sdk: Default::default(),
            uses_feature: Default::default(),
            uses_permission: Default::default(),
            queries: Default::default(),
            application: Default::default(),
        }
    }
}

impl AndroidManifest {
    pub fn write_to(&self, dir: &Path) -> Result<(), NdkError> {
        let file = File::create(dir.join("AndroidManifest.xml"))?;
        let w = std::io::BufWriter::new(file);
        quick_xml::se::to_writer(w, &self)?;
        Ok(())
    }

    pub fn read_from(xml_file: &Path) -> Result<Self, NdkError> {
        use quick_xml::events::{BytesEnd, BytesStart, Event};
        use std::io::Cursor;

        let alt_str = |is_first| {
            if is_first {
                "activity"
            } else {
                "other_activity"
            }
        };

        let xml = std::fs::read_to_string(xml_file)?;
        let mut reader = quick_xml::Reader::from_str(&xml);
        reader.trim_text(true); // XXX: is it needed?

        let mut writer = quick_xml::Writer::new(Cursor::new(Vec::new()));
        let mut is_first_activity = true;
        loop {
            match reader.read_event()? {
                Event::Start(ref e) if e.name().as_ref() == b"activity" => {
                    let mut new_start = BytesStart::new(alt_str(is_first_activity));
                    new_start.extend_attributes(e.attributes().filter_map(Result::ok));
                    writer.write_event(Event::Start(new_start))?;
                }
                Event::End(ref e) if e.name().as_ref() == b"activity" => {
                    let new_end = BytesEnd::new(alt_str(is_first_activity));
                    writer.write_event(Event::End(new_end))?;
                    is_first_activity = false;
                }
                Event::Eof => break,
                e => writer.write_event(e)?,
            }
        }
        let mut buf = writer.into_inner();
        buf.set_position(0);
        let manifest = quick_xml::de::from_reader(buf)?;
        Ok(manifest)
    }
}

/// Android [application element](https://developer.android.com/guide/topics/manifest/application-element), containing an [`Activity`] element.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Application {
    #[serde(rename = "android:debuggable", alias = "debuggable")]
    pub debuggable: Option<bool>,
    #[serde(rename = "android:theme", alias = "theme")]
    pub theme: Option<String>,
    #[serde(rename = "android:hasCode", alias = "has_code")]
    #[serde(default)]
    pub has_code: bool,
    #[serde(rename = "android:icon", alias = "icon")]
    pub icon: Option<String>,
    #[serde(rename = "android:label", alias = "label")]
    #[serde(default)]
    pub label: String,
    #[serde(rename = "android:extractNativeLibs", alias = "extract_native_libs")]
    pub extract_native_libs: Option<bool>,
    #[serde(
        rename = "android:usesCleartextTraffic",
        alias = "uses_cleartext_traffic"
    )]
    pub uses_cleartext_traffic: Option<bool>,

    #[serde(rename = "meta-data", alias = "meta_data")]
    #[serde(default)]
    pub meta_data: Vec<MetaData>,
    /// This is usually the native activity.
    #[serde(default)]
    pub activity: Activity,
    /// Note: this must be handled by `AndroidManifest::read_from` for XML deserialization.
    #[serde(default)]
    #[serde(rename(serialize = "activity", deserialize = "other_activity"))]
    pub other_activities: Vec<Activity>,
}

/// Android [activity element](https://developer.android.com/guide/topics/manifest/activity-element).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Activity {
    #[serde(rename = "android:configChanges", alias = "config_changes")]
    #[serde(default = "default_config_changes")]
    pub config_changes: Option<String>,
    #[serde(rename = "android:label", alias = "label")]
    pub label: Option<String>,
    #[serde(rename = "android:launchMode", alias = "launch_mode")]
    pub launch_mode: Option<String>,
    #[serde(rename = "android:name", alias = "name")]
    #[serde(default = "default_activity_name")]
    pub name: String,
    #[serde(rename = "android:screenOrientation", alias = "orientation")]
    pub orientation: Option<String>,
    #[serde(rename = "android:exported", alias = "exported")]
    pub exported: Option<bool>,
    #[serde(rename = "android:resizeableActivity", alias = "resizeable_activity")]
    pub resizeable_activity: Option<bool>,
    #[serde(
        rename = "android:alwaysRetainTaskState",
        alias = "always_retain_task_state"
    )]
    pub always_retain_task_state: Option<bool>,

    #[serde(rename = "meta-data", alias = "meta_data")]
    #[serde(default)]
    pub meta_data: Vec<MetaData>,
    /// If no `MAIN` action exists in any intent filter, a default `MAIN` filter is serialized by `cargo-apk`.
    #[serde(rename = "intent-filter", alias = "intent_filter")]
    #[serde(default)]
    pub intent_filter: Vec<IntentFilter>,
}

impl Default for Activity {
    fn default() -> Self {
        Self {
            config_changes: default_config_changes(),
            label: None,
            launch_mode: None,
            name: default_activity_name(),
            orientation: None,
            exported: None,
            resizeable_activity: None,
            always_retain_task_state: None,
            meta_data: Default::default(),
            intent_filter: Default::default(),
        }
    }
}

/// Android [intent filter element](https://developer.android.com/guide/topics/manifest/intent-filter-element).
/// Currently deserialization from XML is unsupported.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct IntentFilter {
    /// Serialize strings wrapped in `<action android:name="..." />`
    #[serde(serialize_with = "serialize_actions")]
    #[serde(rename(serialize = "action"))]
    #[serde(default)]
    pub actions: Vec<String>,
    /// Serialize as vector of structs for proper xml formatting
    #[serde(serialize_with = "serialize_catergories")]
    #[serde(rename(serialize = "category"))]
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub data: Vec<IntentFilterData>,
}

fn serialize_actions<S>(actions: &[String], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    use serde::ser::SerializeSeq;

    #[derive(Serialize)]
    struct Action {
        #[serde(rename = "android:name")]
        name: String,
    }
    let mut seq = serializer.serialize_seq(Some(actions.len()))?;
    for action in actions {
        seq.serialize_element(&Action {
            name: action.clone(),
        })?;
    }
    seq.end()
}

fn serialize_catergories<S>(categories: &[String], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    use serde::ser::SerializeSeq;

    #[derive(Serialize)]
    struct Category {
        #[serde(rename = "android:name")]
        pub name: String,
    }

    let mut seq = serializer.serialize_seq(Some(categories.len()))?;
    for category in categories {
        seq.serialize_element(&Category {
            name: category.clone(),
        })?;
    }
    seq.end()
}

/// Android [intent filter data element](https://developer.android.com/guide/topics/manifest/data-element).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct IntentFilterData {
    #[serde(rename = "android:scheme", alias = "scheme")]
    pub scheme: Option<String>,
    #[serde(rename = "android:host", alias = "host")]
    pub host: Option<String>,
    #[serde(rename = "android:port", alias = "port")]
    pub port: Option<String>,
    #[serde(rename = "android:path", alias = "path")]
    pub path: Option<String>,
    #[serde(rename = "android:pathPattern", alias = "path_pattern")]
    pub path_pattern: Option<String>,
    #[serde(rename = "android:pathPrefix", alias = "path_prefix")]
    pub path_prefix: Option<String>,
    #[serde(rename = "android:mimeType", alias = "mime_type")]
    pub mime_type: Option<String>,
}

/// Android [meta-data element](https://developer.android.com/guide/topics/manifest/meta-data-element).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct MetaData {
    #[serde(rename = "android:name", alias = "name")]
    pub name: String,
    #[serde(rename = "android:value", alias = "value")]
    pub value: String,
}

/// Android [uses-feature element](https://developer.android.com/guide/topics/manifest/uses-feature-element).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Feature {
    #[serde(rename = "android:name", alias = "name")]
    pub name: Option<String>,
    #[serde(rename = "android:required", alias = "required")]
    pub required: Option<bool>,
    /// The `version` field is currently used for the following features:
    ///
    /// - `name="android.hardware.vulkan.compute"`: The minimum level of compute features required. See the [Android documentation](https://developer.android.com/reference/android/content/pm/PackageManager#FEATURE_VULKAN_HARDWARE_COMPUTE)
    ///   for available levels and the respective Vulkan features required/provided.
    ///
    /// - `name="android.hardware.vulkan.level"`: The minimum Vulkan requirements. See the [Android documentation](https://developer.android.com/reference/android/content/pm/PackageManager#FEATURE_VULKAN_HARDWARE_LEVEL)
    ///   for available levels and the respective Vulkan features required/provided.
    ///
    /// - `name="android.hardware.vulkan.version"`: Represents the value of Vulkan's `VkPhysicalDeviceProperties::apiVersion`. See the [Android documentation](https://developer.android.com/reference/android/content/pm/PackageManager#FEATURE_VULKAN_HARDWARE_VERSION)
    ///   for available levels and the respective Vulkan features required/provided.
    #[serde(rename = "android:version", alias = "version")]
    pub version: Option<u32>,
    #[serde(rename = "android:glEsVersion", alias = "opengles_version")]
    #[serde(serialize_with = "serialize_opengles_version")]
    pub opengles_version: Option<(u8, u8)>,
}

// XXX: implement a deserializer suitable for XML parsing
fn serialize_opengles_version<S>(
    version: &Option<(u8, u8)>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match version {
        Some(version) => {
            let opengles_version = format!("0x{:04}{:04}", version.0, version.1);
            serializer.serialize_some(&opengles_version)
        }
        None => serializer.serialize_none(),
    }
}

/// Android [uses-permission element](https://developer.android.com/guide/topics/manifest/uses-permission-element).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Permission {
    #[serde(rename = "android:name", alias = "name")]
    pub name: String,
    #[serde(rename = "android:maxSdkVersion", alias = "max_sdk_version")]
    pub max_sdk_version: Option<u32>,
}

/// Android [package element](https://developer.android.com/guide/topics/manifest/queries-element#package).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Package {
    #[serde(rename = "android:name", alias = "name")]
    pub name: String,
}

/// Android [provider element](https://developer.android.com/guide/topics/manifest/queries-element#provider).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct QueryProvider {
    #[serde(rename = "android:authorities", alias = "authorities")]
    pub authorities: String,

    // The specs say only an `authorities` attribute is required for providers contained in a `queries` element
    // however this is required for aapt support and should be made optional if/when cargo-apk migrates to aapt2
    #[serde(rename = "android:name", alias = "name")]
    pub name: String,
}

/// Android [queries element](https://developer.android.com/guide/topics/manifest/queries-element).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Queries {
    #[serde(default)]
    pub package: Vec<Package>,
    #[serde(default)]
    pub intent: Vec<IntentFilter>,
    #[serde(default)]
    pub provider: Vec<QueryProvider>,
}

/// Android [uses-sdk element](https://developer.android.com/guide/topics/manifest/uses-sdk-element).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Sdk {
    #[serde(rename = "android:minSdkVersion", alias = "min_sdk_version")]
    pub min_sdk_version: Option<u32>,
    #[serde(rename = "android:targetSdkVersion", alias = "target_sdk_version")]
    pub target_sdk_version: Option<u32>,
    #[serde(rename = "android:maxSdkVersion", alias = "max_sdk_version")]
    pub max_sdk_version: Option<u32>,
}

impl Default for Sdk {
    fn default() -> Self {
        Self {
            min_sdk_version: Some(23),
            target_sdk_version: None,
            max_sdk_version: None,
        }
    }
}

fn default_namespace() -> String {
    "http://schemas.android.com/apk/res/android".to_string()
}

fn default_activity_name() -> String {
    "android.app.NativeActivity".to_string()
}

fn default_config_changes() -> Option<String> {
    Some("orientation|keyboardHidden|screenSize".to_string())
}
