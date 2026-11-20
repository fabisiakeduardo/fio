//! Reconcile: the decision engine and its driver.
//!
//! The planners are pure functions of the persisted desired state and a
//! snapshot of the audio server's actual state — no libpulse, no I/O — so
//! the entire routing policy is unit-testable without audio hardware. The
//! [`Driver`] wires those plans to the engine: it executes the [`Action`]s
//! a plan returns and surfaces the [`Drift`] flags to the UI.

mod driver;
mod plan;

pub use driver::Driver;
pub use plan::{
    Action, Drift, detect_drift, link_names, link_present, plan_new_stream, plan_startup, rule_key,
};
