//! What every command shares: where files live, the session state machine, the records on
//! disk and the journal of the day.

pub mod journal;
pub mod paths;
pub mod state;
pub mod store;
