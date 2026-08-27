use std::sync::Arc;

use tracing::{info, warn};

use crate::ipc::{
    protocol::{LogEvent, LogLevel},
    session::BroadcastHub,
};

/// Say something once and have it reach both places a daemon message has to
/// appear: the process log, and the `logs` topic that connected clients watch.
///
/// Every corner of the daemon used to write those two lines by hand, one after
/// the other, with the message text spelled out twice — so the operator's
/// terminal and the TUI's log pane were one single-sided edit away from
/// disagreeing about what happened. `cmd_reload_config` had already drifted
/// that way: the file log said "Config reloaded from <path>" while clients were
/// told only "Config reloaded".
///
/// `target` is the module name that goes on the wire in
/// [`LogEvent::target`], which clients filter on, so each owner constructs its
/// relay with the target it used before. The tracing side cannot follow suit —
/// `tracing`'s `target:` has to be a literal — so relayed lines are attributed
/// to this module in the file-log layer. That is a deliberate trade: it shows
/// up only in the file log, and nothing on the IPC contract changes.
pub struct ServerLog {
    hub: Arc<BroadcastHub>,
    target: &'static str,
}

impl ServerLog {
    pub fn new(hub: Arc<BroadcastHub>, target: &'static str) -> Self {
        Self { hub, target }
    }

    /// A milestone: something worked, or is about to.
    pub fn info(&self, message: impl Into<String>) {
        let message = message.into();
        info!("{message}");
        self.publish(LogLevel::Info, message);
    }

    /// Something went wrong, so it reaches a watching client's log pane by the
    /// same route a milestone does instead of only existing in a process log
    /// nobody may be reading.
    pub fn warn(&self, message: impl Into<String>) {
        let message = message.into();
        warn!("{message}");
        self.publish(LogLevel::Warn, message);
    }

    /// Route a message by an already-known level: milestones through
    /// [`Self::info`], everything else through [`Self::warn`]. For the callers
    /// that carry a [`LogLevel`] value instead of choosing a method at the
    /// call site.
    pub fn log(&self, level: LogLevel, message: impl Into<String>) {
        match level {
            LogLevel::Info => self.info(message),
            // The relay only speaks in milestones and warnings — see the two
            // methods above — so a level below Info is voiced as a warning
            // too, exactly as the call sites did before this method existed.
            LogLevel::Trace | LogLevel::Debug | LogLevel::Warn | LogLevel::Error => {
                self.warn(message)
            }
        }
    }

    fn publish(&self, level: LogLevel, message: String) {
        self.hub
            .publish_log(LogEvent::new(level, message).with_target(self.target));
    }
}
