//! A simulated SmartClock receiver.
//!
//! Answers the commands a real one answers, in the shape a real one
//! answers them: character echo, a `scpi > ` prompt, `E-nnn > ` after a
//! bad command, and an error queue that has to be read to clear it.
//! Those are the parts of the protocol that bite, and they are the
//! parts a test needs if it is to mean anything.
//!
//! The point is testing the layers above the wire.  Everything below
//! `Session` can be exercised against a recorded transcript, but the
//! daemon and the socket cannot: they poll on their own schedule and in
//! their own order, so a transcript never matches.  This answers
//! whatever it is asked.

pub mod net;
pub mod receiver;
pub mod transport;
