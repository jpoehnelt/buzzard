//! Verification of state machines found on block diagrams.
//!
//! A state machine here is the classic pattern: a while loop around a case
//! structure, with the case selected by a shift register that each case
//! writes its successor into. [`model`] holds the abstract machine and the
//! checks that are run on it. Extraction from a diagram is a separate step
//! that produces a [`Machine`].
//!
//! # What "verified" means
//!
//! The model is an *over-approximation* of the real program: where a case
//! chooses its successor from data the analysis cannot follow, every state
//! is assumed possible, and a stop condition it cannot read is assumed to
//! be able to go either way. The real program's behaviours are therefore a
//! subset of the model's. That makes the two "cannot happen" results
//! proofs, not heuristics:
//!
//! * a state the model cannot reach is unreachable in the program;
//! * a state from which the model cannot reach a stop cannot terminate in
//!   the program.
//!
//! The reverse does not hold: a state the model *can* reach may still be
//! unreachable in practice. Each result says which kind it is.

pub mod model;

pub use model::{Finding, Machine, Report, State, Stop, Successors};
