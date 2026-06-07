//! CSS subsystem: the value vocabulary ([`values`]), the resolved
//! [`computed`] style layout reads, the [`stylo`] cascade bridge that produces
//! it, and the built-in user-agent stylesheet ([`ua`]).

pub mod computed;
pub mod stylo;
pub mod ua;
pub mod values;
