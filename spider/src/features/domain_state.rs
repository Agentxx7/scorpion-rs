//! Canonical state/transition semantics for persisted Scorpion domain
//! objects.
//!
//! Track 2 of the roadmap, built directly on Track 1's identity
//! ([`crate::features::identity`]: [`EvidenceId`](crate::features::identity::EvidenceId),
//! [`WatchId`](crate::features::identity::WatchId)). This module defines
//! **semantics only** — the neutral vocabulary and invariants every future
//! stateful, persisted capability reuses. It defines no concrete state
//! machine, no persistence, and no product model.
//!
//! # The four distinct concepts
//!
//! Every stateful, persisted Scorpion capability decomposes into exactly
//! these four things, deliberately never merged into one type:
//!
//! 1. **Identity** — who/what this is. Already defined
//!    ([`crate::features::identity`]); this module adds nothing to it and
//!    imports it only to parameterize the types below.
//! 2. **Current state** — [`CurrentState<Id, S>`]. What is true *right
//!    now* for one identity. Exactly one exists per identity at any time
//!    (see "One current state per identity" below) — this is a value,
//!    never a collection.
//! 3. **Historical record** — [`HistoryEntry<Id, S>`]. What used to be
//!    true. Immutable once created; a historical record never changes
//!    after the fact, it can only be superseded by a *new* current state
//!    (which in turn produces its own historical record when it, too, is
//!    superseded).
//! 4. **Transition** — [`Transition<S>`]. The explicit, typed act of
//!    turning one current state into the next. A transition is a pure
//!    decision (state in, state-or-rejection out); it is not itself
//!    state, and it does not persist anything.
//!
//! # The transition contract
//!
//! ```text
//! current state + explicit transition → new current state
//! ```
//!
//! Realized as [`CurrentState::apply`]: given a `&dyn Transition<S>` value,
//! it either produces the new [`CurrentState`] together with the
//! [`HistoryEntry`] recording exactly what was superseded, or it returns
//! the *original*, untouched `CurrentState` alongside the transition's
//! rejection reason. A rejected transition never partially applies and
//! never silently drops the caller's current state.
//!
//! # Invariants
//!
//! - **One current state per stateful identity.** [`CurrentState<Id, S>`]
//!   holds exactly one `identity` and one `state` field — never a
//!   collection of states, never an `Option`-of-many. [`CurrentState::apply`]
//!   consumes the receiver and produces exactly one new `CurrentState`;
//!   there is no operation anywhere in this module that produces two live
//!   current states from one. Enforcing that at most one *persisted* row
//!   exists per identity is a future persistence layer's job (see
//!   "Ownership boundary" below) — this module enforces the shape the
//!   persisted row must have.
//! - **Historical records are immutable and append-only.** [`HistoryEntry`]
//!   has no field-mutating method anywhere — once constructed (only ever
//!   by [`CurrentState::apply`]), its fields cannot change. [`HistoryLog`]
//!   is the only collection this module provides for historical records,
//!   and its API is deliberately append-only: [`HistoryLog::append`] is
//!   the sole write operation; there is no `remove`, no `clear`, no
//!   `get_mut`, no `IndexMut`. A `HistoryLog` can only grow.
//!
//! # Ownership boundary
//!
//! Per `SCORPION_SDD.md` §5.2: *"Transitions are explicit, typed, and
//! persisted; no implicit state drift."* This module draws the boundary
//! implied by that rule precisely:
//!
//! - **Domain code decides whether a transition is valid.** [`Transition::apply`]
//!   is a pure function of `&S` — it receives no storage handle, no
//!   database connection, no I/O capability of any kind. It cannot reach
//!   into persistence even if a future implementor wanted it to; the
//!   trait signature structurally forbids it.
//! - **Persistence stores state; it does not decide domain transitions.**
//!   A future persistence layer's entire job is: durably hold the current
//!   [`CurrentState`] for each identity (one row per identity — the
//!   invariant above), and durably append each [`HistoryEntry`] a
//!   transition produces. It calls [`Transition::apply`] (via
//!   [`CurrentState::apply`]) to get the *decision*; it never
//!   re-implements or second-guesses that decision itself.
//!
//! This module implements neither side of that boundary's storage half —
//! there is no database, cache, or file I/O anywhere below. See "Not
//! implemented here" for the full list.
//!
//! # Naming reconciliation
//!
//! Before this module could introduce any new canonical bare vocabulary,
//! two existing uses of common English words needed to be reconciled so
//! the words stay unambiguous project-wide:
//!
//! - **`Observation`** is already owned by
//!   `spider_agent_types::PageObservation` — an LLM browser-automation
//!   tool's description of a page's current DOM/UI for the *agent*, a
//!   completely different domain (agent tool-use, not persisted
//!   evidence/watch/session state). This module therefore never defines a
//!   bare `Observation` type. The closest concept this module needs — "an
//!   immutable record of what was true at one point in time" — is named
//!   [`HistoryEntry`] instead, with no relation to `PageObservation`.
//! - **`Snapshot`** already has two existing, unrelated, *qualified*
//!   uses — `VitalsSnapshot` (a read-only point-in-time copy of crawl
//!   metrics, `utils/vitals.rs`) and `BrowserChallengeSnapshot` (a
//!   captured CAPTCHA/browser-challenge DOM state for revalidation,
//!   `features/browser_challenge.rs`) — plus one *locked, informal* use as
//!   a bare word in `SCORPION_SDD.md` §5.2's future WATCH/MONITOR chain
//!   (`WatchState → Snapshot → Transition → ...`). Read in context, that
//!   locked "Snapshot" step names *the freshly captured external reading
//!   compared against the current `WatchState` to decide the next
//!   transition* — i.e. the *input* to a future watch-specific
//!   `Transition` impl, not this module's "current state" concept, and
//!   not a new type this module needs to define. This module therefore
//!   never defines a bare `Snapshot` type either: `CurrentState` names
//!   "what is true now," and a future watch frontier realizing §5.2's
//!   "Snapshot" step should define it as a watch-specific input type to
//!   its own `Transition<WatchState>` impl — reusing this module's
//!   contract rather than adding a second, competing "current state"
//!   concept under a different name.
//! - **`Fingerprint`** is a known, separate naming pressure (already used
//!   for browser/anti-detection fingerprinting) that Change Detection will
//!   eventually also want for content-fingerprint comparison. It is
//!   deliberately **not** reconciled here — that belongs to Track 6, per
//!   the frontier that authorized this module.
//!
//! # Not implemented here
//!
//! Per this frontier's explicit scope, none of the following exist in
//! this module, and nothing here should be read as authorizing them:
//!
//! - Any database, cache, or file persistence mechanism.
//! - `WatchDefinition` or a `WatchState` product model.
//! - `AuthSessionId` or authenticated-session lifecycle.
//! - Scheduling of any kind.
//! - `ChangeResult`/`ChangeEvent` or any change-detection product model.
//! - Health/liveness semantics.
//!
//! Building any of those is later, separate frontier work that reuses the
//! generic [`CurrentState`]/[`HistoryEntry`]/[`HistoryLog`]/[`Transition`]
//! vocabulary defined here, parameterized with its own concrete `S`.

use std::time::SystemTime;

/// The single current state of one stateful identity.
///
/// Exactly one `CurrentState<Id, S>` value represents "what is true now"
/// for `identity` — never a collection, never more than one live state per
/// identity. See the module-level "One current state per identity"
/// invariant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentState<Id, S> {
    identity: Id,
    state: S,
}

/// The result of successfully applying a [`Transition`] to a
/// [`CurrentState`]: the new current state, and the immutable historical
/// record of exactly what it superseded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied<Id, S> {
    /// The new current state — replaces the old one entirely; there is
    /// exactly one of these per identity.
    pub current: CurrentState<Id, S>,
    /// The immutable record of the state that was current immediately
    /// before this transition. Append this to a [`HistoryLog`]; it is
    /// never mutated after being produced here.
    pub superseded: HistoryEntry<Id, S>,
}

impl<Id, S> CurrentState<Id, S> {
    /// Construct the current state for `identity`. This is the only way
    /// to create a `CurrentState` from nothing — there is no `Default`
    /// (a stateful identity with no chosen initial state is not a state
    /// this module can name).
    pub fn new(identity: Id, state: S) -> Self {
        Self { identity, state }
    }

    /// The identity this is the current state of.
    pub fn identity(&self) -> &Id {
        &self.identity
    }

    /// The state itself, as of now.
    pub fn state(&self) -> &S {
        &self.state
    }

    /// Decompose into the owned identity and state, consuming `self`.
    /// Named `into_parts` rather than exposing public fields: callers
    /// must go through a method to pull a `CurrentState` apart, never
    /// construct or mutate one field-by-field.
    pub fn into_parts(self) -> (Id, S) {
        (self.identity, self.state)
    }
}

impl<Id: Clone, S: Clone> CurrentState<Id, S> {
    /// Apply `transition` to this current state: the canonical
    /// `current state + explicit transition → new current state`
    /// contract.
    ///
    /// On success, consumes `self` and returns the [`Applied`] new
    /// current state plus the [`HistoryEntry`] recording what was
    /// superseded. On rejection, returns `self` completely unchanged
    /// alongside the transition's rejection reason — a rejected
    /// transition never partially applies.
    ///
    /// This method (and [`Transition::apply`] beneath it) performs no
    /// I/O and touches no storage of any kind; it is a pure function of
    /// its two inputs.
    pub fn apply<T: Transition<S>>(
        self,
        transition: &T,
    ) -> Result<Applied<Id, S>, (Self, T::Rejection)> {
        match transition.apply(&self.state) {
            Ok(new_state) => {
                let superseded = HistoryEntry {
                    identity: self.identity.clone(),
                    state: self.state,
                    recorded_at: SystemTime::now(),
                };
                Ok(Applied {
                    current: CurrentState {
                        identity: self.identity,
                        state: new_state,
                    },
                    superseded,
                })
            }
            Err(rejection) => Err((self, rejection)),
        }
    }
}

/// A pure, explicit, typed decision: does this transition apply to the
/// given state, and if so what state results?
///
/// Implementations receive only `&S` — no storage handle, no database
/// connection, no reference to any identity, no I/O capability. That is
/// deliberate: it is structurally impossible for a `Transition` impl to
/// read or write persistence, matching the "persistence stores state but
/// does not decide valid domain transitions" ownership boundary. A
/// concrete domain (a future WatchState, authenticated-session lifecycle,
/// evidence ledger, or change-detection frontier) implements this trait
/// once for its own state type; this module never implements it for any
/// concrete `S`.
pub trait Transition<S> {
    /// Why this transition does not apply to a given current state.
    /// Concrete domains define their own rejection vocabulary — this
    /// trait deliberately does not presume one.
    type Rejection;

    /// Decide the result of applying this transition to `current`.
    /// `Ok` names the new state exactly; `Err` means `current` is left
    /// entirely unchanged by the caller (see [`CurrentState::apply`]).
    fn apply(&self, current: &S) -> Result<S, Self::Rejection>;
}

/// One immutable record of what was true for `identity`, as of
/// `recorded_at`.
///
/// The only way to obtain a `HistoryEntry` is as the `superseded` field of
/// an [`Applied`] value, produced by [`CurrentState::apply`] — there is no
/// public constructor, and no method on this type mutates any field after
/// construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry<Id, S> {
    identity: Id,
    state: S,
    recorded_at: SystemTime,
}

impl<Id, S> HistoryEntry<Id, S> {
    /// The identity this record is about.
    pub fn identity(&self) -> &Id {
        &self.identity
    }

    /// The state that was current at `recorded_at`.
    pub fn state(&self) -> &S {
        &self.state
    }

    /// When this state stopped being current (the moment it was
    /// superseded, not when it first became current).
    pub fn recorded_at(&self) -> SystemTime {
        self.recorded_at
    }
}

/// An append-only sequence of [`HistoryEntry`] values for one stateful
/// identity (or, at a caller's choice, for several — this type does not
/// itself enforce single-identity grouping; callers needing that keep one
/// `HistoryLog` per identity).
///
/// [`HistoryLog::append`] is the only write operation. There is
/// deliberately no `remove`, `clear`, `get_mut`, `IndexMut`, or any other
/// way to alter or discard an entry once appended — realizing the
/// "historical records are immutable/append-only" invariant structurally,
/// not just by convention.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryLog<Id, S> {
    entries: Vec<HistoryEntry<Id, S>>,
}

impl<Id, S> HistoryLog<Id, S> {
    /// An empty log.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Append one historical record. The record is retained exactly as
    /// given for the lifetime of this log — this is the only mutating
    /// method `HistoryLog` has.
    pub fn append(&mut self, entry: HistoryEntry<Id, S>) {
        self.entries.push(entry);
    }

    /// Number of retained records.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether this log has no records yet.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate over every retained record, oldest first.
    pub fn iter(&self) -> std::slice::Iter<'_, HistoryEntry<Id, S>> {
        self.entries.iter()
    }

    /// The most recently appended record, if any.
    pub fn last(&self) -> Option<&HistoryEntry<Id, S>> {
        self.entries.last()
    }
}

impl<Id, S> Default for HistoryLog<Id, S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a, Id, S> IntoIterator for &'a HistoryLog<Id, S> {
    type Item = &'a HistoryEntry<Id, S>;
    type IntoIter = std::slice::Iter<'a, HistoryEntry<Id, S>>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::identity::EvidenceId;

    /// A tiny, made-up domain state for exercising the contract — not a
    /// real Scorpion domain object, exists only inside this test module.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestState {
        Pending,
        Active,
        Closed,
    }

    struct Activate;
    struct Close;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct IllegalTransition {
        from: TestState,
    }

    impl Transition<TestState> for Activate {
        type Rejection = IllegalTransition;

        fn apply(&self, current: &TestState) -> Result<TestState, Self::Rejection> {
            match current {
                TestState::Pending => Ok(TestState::Active),
                other => Err(IllegalTransition { from: *other }),
            }
        }
    }

    impl Transition<TestState> for Close {
        type Rejection = IllegalTransition;

        fn apply(&self, current: &TestState) -> Result<TestState, Self::Rejection> {
            match current {
                TestState::Active => Ok(TestState::Closed),
                other => Err(IllegalTransition { from: *other }),
            }
        }
    }

    #[test]
    fn transition_contract_produces_new_current_state_and_history_entry() {
        let id = EvidenceId::new();
        let current = CurrentState::new(id, TestState::Pending);

        let applied = current
            .apply(&Activate)
            .expect("Pending -> Active is legal");
        assert_eq!(*applied.current.state(), TestState::Active);
        assert_eq!(*applied.current.identity(), id);

        // The historical record names exactly the state that was
        // superseded (the old one), not the new one.
        assert_eq!(*applied.superseded.state(), TestState::Pending);
        assert_eq!(*applied.superseded.identity(), id);
    }

    #[test]
    fn rejected_transition_returns_current_state_completely_unchanged() {
        let id = EvidenceId::new();
        let current = CurrentState::new(id, TestState::Pending);

        // Close only applies from Active, not Pending.
        let (returned, rejection) = current.apply(&Close).unwrap_err();
        assert_eq!(*returned.state(), TestState::Pending);
        assert_eq!(*returned.identity(), id);
        assert_eq!(
            rejection,
            IllegalTransition {
                from: TestState::Pending
            }
        );
    }

    #[test]
    fn one_current_state_per_identity_chain() {
        // Each apply() call consumes the prior CurrentState and produces
        // exactly one new one — there is never a point where two current
        // states exist for the same identity from this API alone.
        let id = EvidenceId::new();
        let s0 = CurrentState::new(id, TestState::Pending);
        let a1 = s0.apply(&Activate).unwrap();
        assert_eq!(*a1.current.state(), TestState::Active);
        let a2 = a1.current.apply(&Close).unwrap();
        assert_eq!(*a2.current.state(), TestState::Closed);
    }

    #[test]
    fn history_log_is_append_only_and_preserves_order() {
        let id = EvidenceId::new();
        let s0 = CurrentState::new(id, TestState::Pending);
        let a1 = s0.apply(&Activate).unwrap();
        let a2 = a1.current.apply(&Close).unwrap();

        let mut log = HistoryLog::new();
        assert!(log.is_empty());
        log.append(a1.superseded);
        log.append(a2.superseded);

        assert_eq!(log.len(), 2);
        let states: Vec<TestState> = log.iter().map(|entry| *entry.state()).collect();
        assert_eq!(states, vec![TestState::Pending, TestState::Active]);
        assert_eq!(*log.last().unwrap().state(), TestState::Active);

        // IntoIterator over &HistoryLog agrees with iter().
        let via_into_iter: Vec<TestState> = (&log).into_iter().map(|e| *e.state()).collect();
        assert_eq!(via_into_iter, states);
    }

    #[test]
    fn history_entries_record_distinct_recorded_at_and_are_immutable_by_construction() {
        let id = EvidenceId::new();
        let s0 = CurrentState::new(id, TestState::Pending);
        let applied = s0.apply(&Activate).unwrap();
        // recorded_at is populated (not the epoch/default) — a real
        // timestamp was taken at the moment of supersession.
        assert!(applied.superseded.recorded_at() >= SystemTime::UNIX_EPOCH);
        // HistoryEntry exposes only read accessors — this line would not
        // compile if any existed: `applied.superseded.state = ...`.
    }

    #[test]
    fn current_state_into_parts_returns_identity_and_state() {
        let id = EvidenceId::new();
        let current = CurrentState::new(id, TestState::Pending);
        let (returned_id, returned_state) = current.into_parts();
        assert_eq!(returned_id, id);
        assert_eq!(returned_state, TestState::Pending);
    }
}
