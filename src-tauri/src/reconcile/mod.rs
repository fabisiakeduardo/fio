//! Reconcile: the pure decision engine.
//!
//! Everything in this module is a pure function of the persisted desired
//! state and a snapshot of the audio server's actual state — no libpulse,
//! no I/O — so the entire routing policy is unit-testable without audio
//! hardware. The driver wires these plans to the engine: it executes the
//! [`Action`]s a plan returns and surfaces the [`Drift`] flags to the UI.

mod plan;

pub use plan::{Action, Drift, detect_drift, plan_new_stream, plan_startup, rule_key};
