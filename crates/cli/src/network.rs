//! Named connection budgets for ordinary and constrained networks.
use clap::ValueEnum;
use lightremote_core::link::LinkOptions;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub(crate) enum Profile {
    #[default]
    Standard,
    Slow,
}
impl Profile {
    pub(crate) fn rpc_budget(self) -> Duration {
        match self {
            Self::Standard => Duration::from_secs(25),
            Self::Slow => Duration::from_secs(180),
        }
    }
    pub(crate) fn options(self) -> LinkOptions {
        let mut options = LinkOptions::default();
        if matches!(self, Self::Slow) {
            options.connect_timeout = Duration::from_secs(180);
            options.open_timeout = Duration::from_secs(60);
            options.write_timeout = Duration::from_secs(90);
            options.heartbeat_timeout = Duration::from_secs(90);
            options.degraded_after = Duration::from_secs(5);
            options.drain_timeout = Duration::from_secs(120);
        }
        options
    }
}
