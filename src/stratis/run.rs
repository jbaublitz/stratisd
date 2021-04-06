// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! Main loop

use std::{
    os::unix::io::AsRawFd,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};

use futures::executor::block_on;
use libudev::EventType;
use nix::poll::{poll, PollFd, PollFlags};
use tokio::{
    runtime::Builder,
    select, signal,
    sync::{
        mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
        Mutex,
    },
    task,
};

use devicemapper::DmNameBuf;

use crate::{
    engine::{Engine, SimEngine, StratEngine, UdevEngineEvent},
    stratis::{
        errors::{StratisError, StratisResult},
        ipc_support::setup,
        stratis::VERSION,
        udev_monitor::UdevMonitor,
    },
};

async fn dm_thread(
    engine: Arc<Mutex<dyn Engine>>,
    mut recv: UnboundedReceiver<String>,
) -> StratisResult<()> {
    loop {
        let dm_name = recv.recv().await.ok_or_else(|| {
            StratisError::Error(
                "The channel between the udev thread and devicemapper thread was closed"
                    .to_string(),
            )
        })?;
        engine.lock().await.evented(DmNameBuf::new(dm_name)?)?;
    }
}

async fn send_uevent(
    dbus_sender: &mut UnboundedSender<UdevEngineEvent>,
    dm_sender: &mut UnboundedSender<String>,
    event: UdevEngineEvent,
) {
    if event.event_type() == EventType::Change {
        if let Some(dm_name) = event.device().property_value("DM_NAME") {
            let dm_name_string = match dm_name.to_str().map(|s| s.to_string()) {
                Some(s) => s,
                None => {
                    warn!("Could not convert udev variable value for DM_NAME to a string");
                    return;
                }
            };
            if let Err(e) = dm_sender.send(dm_name_string) {
                warn!("Failed to notify the engine of a devicemapper event; the devicemapper information may be stale as a result: {}", e);
            }
        }
    } else if let Err(e) = dbus_sender.send(event) {
        warn!("Failed to notify the engine of a udev event; the engine may not be aware of a new device: {}", e);
    }
}

// Poll for udev events.
// Check for exit condition and return if true.
fn udev_thread(
    mut dbus_sender: UnboundedSender<UdevEngineEvent>,
    mut dm_sender: UnboundedSender<String>,
    should_exit: Arc<AtomicBool>,
) -> StratisResult<()> {
    let context = libudev::Context::new()?;
    let mut udev = UdevMonitor::create(&context)?;

    let mut pollers = [PollFd::new(udev.as_raw_fd(), PollFlags::POLLIN)];
    loop {
        match poll(&mut pollers, 100)? {
            0 => {
                if should_exit.load(Ordering::Relaxed) {
                    info!("udev thread was notified to exit");
                    return Ok(());
                }
            }
            _ => {
                if let Some(ref e) = udev.poll() {
                    block_on(send_uevent(
                        &mut dbus_sender,
                        &mut dm_sender,
                        UdevEngineEvent::from(e),
                    ))
                }
            }
        }
    }
}

// Waits for SIGINT. If received, sets should_exit to true.
async fn signal_thread(should_exit: Arc<AtomicBool>) {
    if let Err(e) = signal::ctrl_c().await {
        error!("Failure while listening for signals: {}", e);
    }
    should_exit.store(true, Ordering::Relaxed);
}

/// Set up all sorts of signal and event handling mechanisms.
/// Initialize the engine and keep it running until a signal is received
/// or a fatal error is encountered.
/// If sim is true, start the sim engine rather than the real engine.
/// Always check for devicemapper context.
pub fn run(sim: bool) -> StratisResult<()> {
    let runtime = Builder::new_multi_thread()
        .enable_all()
        .thread_name_fn(|| {
            static ATOMIC_ID: AtomicUsize = AtomicUsize::new(0);
            let id = ATOMIC_ID.fetch_add(1, Ordering::SeqCst);
            format!("stratis-wt-{}", id)
        })
        .on_thread_start(|| {
            static ATOMIC_ID: AtomicUsize = AtomicUsize::new(0);
            let id = ATOMIC_ID.fetch_add(1, Ordering::SeqCst);
            debug!("{}: thread started", id)
        })
        .on_thread_stop(|| {
            static ATOMIC_ID: AtomicUsize = AtomicUsize::new(0);
            let id = ATOMIC_ID.fetch_add(1, Ordering::SeqCst);
            debug!("{}: thread finished", id)
        })
        .build()?;
    runtime.block_on(async move {
        let engine: Arc<Mutex<dyn Engine>> = {
            info!("stratis daemon version {} started", VERSION);
            if sim {
                info!("Using SimEngine");
                Arc::new(Mutex::new(SimEngine::default()))
            } else {
                info!("Using StratEngine");
                Arc::new(Mutex::new(match StratEngine::initialize() {
                    Ok(engine) => engine,
                    Err(e) => {
                        error!("Failed to start up stratisd engine: {}; exiting", e);
                        return;
                    }
                }))
            }
        };

        let should_exit = Arc::new(AtomicBool::new(false));
        let (dbus_sender, dbus_receiver) = unbounded_channel::<UdevEngineEvent>();
        let (dm_sender, dm_receiver) = unbounded_channel::<String>();

        let udev_arc_clone = Arc::clone(&should_exit);
        let join_udev = task::spawn_blocking(move || udev_thread(dbus_sender, dm_sender, udev_arc_clone));
        let join_dm = task::spawn(dm_thread(Arc::clone(&engine), dm_receiver));
        let join_ipc = task::spawn(setup(Arc::clone(&engine), dbus_receiver));
        let join_signal = task::spawn(signal_thread(Arc::clone(&should_exit)));

        select! {
            res = join_udev => {
                if let Ok(Err(e)) = res {
                    error!("The udev thread exited with an error: {}; shutting down stratisd...", e);
                } else {
                    error!("The udev thread exited; shutting down stratisd...");
                }
            }
            res = join_dm => {
                if let Ok(Err(e)) = res {
                    error!("The devicemapper event processing thread exited with an error: {}; shutting down stratisd...", e);
                } else {
                    error!("The devicemapper event processing thread exited; shutting down stratisd...");
                }
            }
            res = join_ipc => {
                if let Ok(Err(e)) = res {
                    error!("The IPC thread exited with an error: {}; shutting down stratisd...", e);
                } else {
                    error!("The IPC thread exited; shutting down stratisd...");
                }
            }
            _ = join_signal => {
                info!("Caught SIGINT; exiting...");
            }
        }
        should_exit.store(true, Ordering::Relaxed);
    });
    Ok(())
}
