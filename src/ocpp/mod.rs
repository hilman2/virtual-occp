//! OCPP message definitions for 1.6-J and 2.0.1.
//!
//! We define the messages directly via serde (instead of an external crate), since the
//! simulator only needs a handful of actions. JSON field names match the official
//! Open Charge Alliance schemas exactly.

pub mod v16;
pub mod v201;
