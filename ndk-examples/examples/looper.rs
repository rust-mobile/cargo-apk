//! Demonstrates how to manage application lifetime using Android's `Looper`

use std::os::fd::AsFd;
use std::time::Duration;

use android_activity::{AndroidApp, InputStatus, MainEvent, PollEvent};
use log::info;
use ndk::looper::{FdEvent, ThreadLooper};

#[no_mangle]
fn android_main(app: AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info),
    );

    // Retrieve the Looper that android-activity created for us on the current thread.
    // android-activity uses this to block on events and poll file descriptors with a single mechanism.
    let looper =
        ThreadLooper::for_thread().expect("ndk-glue did not attach thread looper before main()!");

    // Create a Unix pipe to send custom events to the Looper. ndk-glue uses a similar mechanism to deliver
    // ANativeActivityCallbacks asynchronously to the Looper through NDK_GLUE_LOOPER_EVENT_PIPE_IDENT.
    let custom_event_pipe = rustix::pipe::pipe().unwrap();
    let custom_callback_pipe = rustix::pipe::pipe().unwrap();

    // Attach the reading end of a pipe to a callback, too
    looper
        .add_fd_with_callback(
            custom_callback_pipe.0.as_fd(),
            FdEvent::INPUT,
            |fd, _event| {
                let mut recv = (!0u32).to_le_bytes();
                assert_eq!(rustix::io::read(fd, &mut recv).unwrap(), size_of_val(&recv));
                let recv = u32::from_le_bytes(recv);
                println!("Read custom event from pipe, in callback: {recv}");
                // Detach this handler by returning `false` once the count reaches 5
                recv < 5
            },
        )
        .expect("Failed to add file descriptor to Looper");

    std::thread::spawn(move || {
        // Send a "custom event" to the looper every second
        for i in 0u32.. {
            std::thread::sleep(Duration::from_secs(1));
            assert_eq!(
                rustix::io::write(&custom_event_pipe.1, &i.to_le_bytes()).unwrap(),
                size_of_val(&i)
            );
            assert_eq!(
                rustix::io::write(&custom_callback_pipe.1, &i.to_le_bytes()).unwrap(),
                size_of_val(&i)
            );
        }
    });

    let mut exit = false;
    let mut redraw_pending = true;
    let mut render_state: Option<()> = Default::default();

    while !exit {
        app.poll_events(
            Some(std::time::Duration::from_secs(1)), /* timeout */
            |event| {
                match event {
                    PollEvent::Wake => {
                        info!("Early wake up");
                    }
                    PollEvent::Timeout => {
                        info!("Timed out");
                        // Real app would probably rely on vblank sync via graphics API...
                        redraw_pending = true;
                    }
                    PollEvent::Main(main_event) => {
                        info!("Main event: {main_event:?}");
                        match main_event {
                            MainEvent::SaveState { saver, .. } => {
                                saver.store("foo://bar".as_bytes());
                            }
                            MainEvent::Pause => {}
                            MainEvent::Resume { loader, .. } => {
                                if let Some(state) = loader.load() {
                                    if let Ok(uri) = String::from_utf8(state) {
                                        info!("Resumed with saved state = {uri:#?}");
                                    }
                                }
                            }
                            MainEvent::InitWindow { .. } => {
                                render_state = Some(());
                                redraw_pending = true;
                            }
                            MainEvent::TerminateWindow { .. } => {
                                render_state = None;
                            }
                            MainEvent::WindowResized { .. } => {
                                redraw_pending = true;
                            }
                            MainEvent::RedrawNeeded { .. } => {
                                redraw_pending = true;
                            }
                            MainEvent::InputAvailable { .. } => {
                                redraw_pending = true;
                            }
                            MainEvent::ConfigChanged { .. } => {
                                info!("Config Changed: {:#?}", app.config());
                            }
                            MainEvent::LowMemory => {}

                            MainEvent::Destroy => exit = true,
                            _ => { /* ... */ }
                        }
                    }
                    _ => {}
                }

                if redraw_pending {
                    if let Some(_rs) = render_state {
                        redraw_pending = false;

                        // Handle input
                        while app.input_events_iter().unwrap().next(|event| {
                            info!("Input Event: {event:?}");
                            InputStatus::Unhandled
                        }) {}

                        info!("Render...");
                    }
                }
            },
        );
    }
}
