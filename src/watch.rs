//! STUB (first pass): EE.log tail loop is explicitly OUT of scope.
//!
//! The live watcher (offset-tracked reads, `notify` wakeups + 2s poll
//! fallback, truncate/recreate survival per BRIEF §5/`watch` and §3)
//! runs against a real game log, which does not exist on this box.
//! Do not half-build it: the event consumer it would feed (`elog`)
//! is implemented and unit-tested against synthetic lines.
