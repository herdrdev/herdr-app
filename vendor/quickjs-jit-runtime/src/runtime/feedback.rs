use std::collections::{btree_map::Entry as MapEntry, BTreeMap};

use std::sync::{Arc, OnceLock};

use super::FunctionKey;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum FeedbackKind {
    Value,
    CallTarget,
    Exit,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ObservedType {
    Int32,
    Float64,
    Bool,
    Null,
    Undefined,
    String,
    Object,
    Function(FunctionKey),
    BigInt,
    Symbol,
}

/// Native value representations currently supported by bounded Tier 2 versions.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum FeedbackRepresentation {
    Int32,
    Float64,
    /// A tagged QuickJS heap reference. Tier 2 keeps the tagged payload and
    /// guards the object tag at entry; element-specific class guards follow.
    HeapRef,
    /// A Bool argument in a tagged frame or guarded primitive direct-call entry.
    Bool,
}

/// Prevent feedback instability from creating an unbounded signature key.
pub const MAX_SPECIALIZED_ARGUMENTS: usize = 8;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct BoundedSpecializationSignature {
    function: FunctionKey,
    arguments: Box<[FeedbackRepresentation]>,
    result: FeedbackRepresentation,
    feedback_epoch: u64,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CallSpecializationKey {
    caller: FunctionKey,
    callee: FunctionKey,
    arguments: Box<[FeedbackRepresentation]>,
    result: FeedbackRepresentation,
    feedback_epoch: u64,
    callee_identity: u64,
    callee_bytecode_identity: u64,
}

/// One observed call target, independent of argument and result representation.
/// This fixed-size status neither pins executable code nor proves call kind,
/// arity, purity, or native-entry readiness. Dispatch must guard its identities.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CallLinkStatus {
    caller: FunctionKey,
    pc: u32,
    callee: FunctionKey,
    callee_identity: u64,
    callee_bytecode_identity: u64,
}

impl CallLinkStatus {
    pub const fn caller(&self) -> FunctionKey {
        self.caller
    }
    pub const fn pc(&self) -> u32 {
        self.pc
    }
    pub const fn callee(&self) -> FunctionKey {
        self.callee
    }
    pub const fn callee_identity(&self) -> u64 {
        self.callee_identity
    }
    pub const fn callee_bytecode_identity(&self) -> u64 {
        self.callee_bytecode_identity
    }
}

impl CallSpecializationKey {
    pub const fn caller(&self) -> FunctionKey {
        self.caller
    }
    pub const fn callee(&self) -> FunctionKey {
        self.callee
    }
    pub fn arity(&self) -> usize {
        self.arguments.len()
    }
    pub fn arguments(&self) -> &[FeedbackRepresentation] {
        &self.arguments
    }
    pub const fn result(&self) -> FeedbackRepresentation {
        self.result
    }
    pub const fn feedback_epoch(&self) -> u64 {
        self.feedback_epoch
    }
    pub const fn callee_identity(&self) -> u64 {
        self.callee_identity
    }
    pub const fn callee_bytecode_identity(&self) -> u64 {
        self.callee_bytecode_identity
    }
}

impl BoundedSpecializationSignature {
    pub const fn function(&self) -> FunctionKey {
        self.function
    }
    pub const fn generation(&self) -> u64 {
        self.function.generation
    }
    pub fn arity(&self) -> usize {
        self.arguments.len()
    }
    pub fn arguments(&self) -> &[FeedbackRepresentation] {
        &self.arguments
    }
    pub const fn result(&self) -> FeedbackRepresentation {
        self.result
    }
    pub const fn feedback_epoch(&self) -> u64 {
        self.feedback_epoch
    }

    /// Stable across processes and Rust releases; suitable for artifact keys.
    pub fn fingerprint(&self) -> u64 {
        fn mix(state: u64, value: u64) -> u64 {
            (state ^ value).wrapping_mul(0x100_0000_01b3)
        }
        let mut state = 0xcbf2_9ce4_8422_2325;
        state = mix(state, self.function.id);
        state = mix(state, self.function.generation);
        state = mix(state, self.arguments.len() as u64);
        for argument in &self.arguments {
            state = mix(state, representation_tag(*argument));
        }
        state = mix(state, representation_tag(self.result));
        mix(state, self.feedback_epoch)
    }
}

const fn representation_tag(representation: FeedbackRepresentation) -> u64 {
    match representation {
        FeedbackRepresentation::Int32 => 1,
        FeedbackRepresentation::Float64 => 2,
        FeedbackRepresentation::HeapRef => 3,
        FeedbackRepresentation::Bool => 4,
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BinaryFeedbackFlags(u32);

impl BinaryFeedbackFlags {
    pub const NONE: Self = Self(0);
    pub const OVERFLOW: Self = Self(1 << 0);
    pub const NEGATIVE_ZERO: Self = Self(1 << 1);
    pub const NAN: Self = Self(1 << 2);

    pub const fn bits(self) -> u32 {
        self.0
    }
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl core::ops::BitOrAssign for BinaryFeedbackFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FeedbackState {
    Monomorphic,
    Polymorphic,
    Megamorphic,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct FeedbackKey {
    function: FunctionKey,
    pc: u32,
    kind: FeedbackKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Entry {
    observations: Vec<ObservedType>,
    megamorphic: bool,
}

impl Entry {
    fn observe(&mut self, observation: ObservedType, diversity_limit: usize) -> FeedbackState {
        if self.megamorphic {
            return FeedbackState::Megamorphic;
        }
        if !self.observations.contains(&observation) {
            if self.observations.len() >= diversity_limit {
                self.megamorphic = true;
                self.observations.clear();
                return FeedbackState::Megamorphic;
            }
            self.observations.push(observation);
        }
        if self.observations.len() == 1 {
            FeedbackState::Monomorphic
        } else {
            FeedbackState::Polymorphic
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeedbackSnapshotEntry {
    function: FunctionKey,
    pc: u32,
    kind: FeedbackKind,
    state: FeedbackState,
    observations: Box<[ObservedType]>,
}

impl FeedbackSnapshotEntry {
    pub const fn function(&self) -> FunctionKey {
        self.function
    }
    pub const fn pc(&self) -> u32 {
        self.pc
    }
    pub const fn kind(&self) -> FeedbackKind {
        self.kind
    }
    pub const fn state(&self) -> FeedbackState {
        self.state
    }
    pub fn observations(&self) -> &[ObservedType] {
        &self.observations
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeedbackSnapshot {
    epoch: u64,
    global: Arc<GlobalFeedback>,
    properties: Box<[(u32, super::ShapeFeedbackSite)]>,
}

#[derive(Debug, Default, Eq, PartialEq)]
struct GlobalFeedback {
    entries: Box<[FeedbackSnapshotEntry]>,
    calls: Box<[CallFeedbackSnapshot]>,
    binaries: Box<[BinaryFeedbackSnapshot]>,
    conversions: Box<[ConversionFeedbackSnapshot]>,
    branches: Box<[BranchFeedbackSnapshot]>,
    call_signatures: Box<[CallSignatureFeedbackSnapshot]>,
    arrays: Box<[super::ArrayFeedbackSnapshot]>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallSignatureFeedbackSnapshot {
    caller: FunctionKey,
    pc: u32,
    state: FeedbackState,
    target_state: FeedbackState,
    targets: Box<[FunctionKey]>,
    stable_arity: Option<usize>,
    arguments: Box<[Box<[ObservedType]>]>,
    results: Box<[ObservedType]>,
    callee_identity: u64,
    callee_bytecode_identity: u64,
}

impl CallSignatureFeedbackSnapshot {
    pub const fn state(&self) -> FeedbackState {
        self.state
    }
    pub const fn target_state(&self) -> FeedbackState {
        self.target_state
    }
    pub fn targets(&self) -> &[FunctionKey] {
        &self.targets
    }
    pub fn argument(&self, index: usize) -> &[ObservedType] {
        self.arguments.get(index).map_or(&[], Box::as_ref)
    }
    pub fn results(&self) -> &[ObservedType] {
        &self.results
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversionFeedbackSnapshot {
    function: FunctionKey,
    pc: u32,
    state: FeedbackState,
    operand: Box<[ObservedType]>,
    result: Box<[ObservedType]>,
}

impl ConversionFeedbackSnapshot {
    pub const fn state(&self) -> FeedbackState {
        self.state
    }
    pub fn operand(&self) -> &[ObservedType] {
        &self.operand
    }
    pub fn result(&self) -> &[ObservedType] {
        &self.result
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BranchFeedbackSnapshot {
    function: FunctionKey,
    pc: u32,
    state: FeedbackState,
    condition_types: Box<[ObservedType]>,
    outcomes: u8,
}

impl BranchFeedbackSnapshot {
    pub const fn state(&self) -> FeedbackState {
        self.state
    }
    pub fn condition_types(&self) -> &[ObservedType] {
        &self.condition_types
    }
    pub const fn was_taken(&self) -> bool {
        self.outcomes & 1 != 0
    }
    pub const fn was_not_taken(&self) -> bool {
        self.outcomes & 2 != 0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryFeedbackSnapshot {
    function: FunctionKey,
    pc: u32,
    state: FeedbackState,
    lhs: Box<[ObservedType]>,
    rhs: Box<[ObservedType]>,
    result: Box<[ObservedType]>,
    flags: BinaryFeedbackFlags,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallFeedbackSnapshot {
    function: FunctionKey,
    state: FeedbackState,
    arguments: Box<[Box<[ObservedType]>]>,
    stable_types: Option<Box<[ObservedType]>>,
    stable_arity: Option<usize>,
}

impl CallFeedbackSnapshot {
    pub const fn state(&self) -> FeedbackState {
        self.state
    }
    pub fn argc(&self) -> usize {
        self.arguments.len()
    }
    pub fn argument(&self, index: usize) -> &[ObservedType] {
        self.arguments.get(index).map_or(&[], Box::as_ref)
    }
}

impl BinaryFeedbackSnapshot {
    pub const fn state(&self) -> FeedbackState {
        self.state
    }
    pub fn lhs(&self) -> &[ObservedType] {
        &self.lhs
    }
    pub fn rhs(&self) -> &[ObservedType] {
        &self.rhs
    }
    pub fn result(&self) -> &[ObservedType] {
        &self.result
    }
    pub const fn flags(&self) -> BinaryFeedbackFlags {
        self.flags
    }
}

impl FeedbackSnapshot {
    pub fn empty(epoch: u64) -> Self {
        static EMPTY: OnceLock<Arc<GlobalFeedback>> = OnceLock::new();
        Self {
            epoch,
            global: EMPTY.get_or_init(Arc::default).clone(),
            properties: Box::new([]),
        }
    }
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn entries(&self) -> &[FeedbackSnapshotEntry] {
        &self.global.entries
    }
    pub fn with_properties(mut self, properties: Vec<(u32, super::ShapeFeedbackSite)>) -> Self {
        self.properties = properties.into_boxed_slice();
        self
    }
    pub fn property_at(&self, pc: u32) -> Option<&super::ShapeFeedbackSite> {
        self.properties
            .iter()
            .find_map(|(p, site)| (*p == pc).then_some(site))
    }

    pub fn arrays(&self) -> &[super::ArrayFeedbackSnapshot] {
        &self.global.arrays
    }

    pub fn array_at(
        &self,
        function: FunctionKey,
        pc: u32,
    ) -> Option<&super::ArrayFeedbackSnapshot> {
        self.global
            .arrays
            .iter()
            .find(|site| site.function() == function && site.pc() == pc)
    }

    /// Additional retained snapshot storage for bounded array profiles.
    pub fn array_feedback_bytes(&self) -> usize {
        core::mem::size_of_val(self.global.arrays.as_ref())
    }

    pub fn function(&self) -> Option<FunctionKey> {
        let function = self
            .global
            .entries
            .first()
            .map(|entry| entry.function)
            .or_else(|| self.global.calls.first().map(|entry| entry.function))
            .or_else(|| self.global.binaries.first().map(|entry| entry.function))
            .or_else(|| self.global.conversions.first().map(|entry| entry.function))
            .or_else(|| self.global.branches.first().map(|entry| entry.function))
            .or_else(|| {
                self.global
                    .call_signatures
                    .first()
                    .map(|entry| entry.caller)
            })
            .or_else(|| self.global.arrays.first().map(|entry| entry.function()))?;
        let same = self
            .global
            .entries
            .iter()
            .all(|entry| entry.function == function)
            && self
                .global
                .calls
                .iter()
                .all(|entry| entry.function == function)
            && self
                .global
                .binaries
                .iter()
                .all(|entry| entry.function == function)
            && self
                .global
                .conversions
                .iter()
                .all(|entry| entry.function == function)
            && self
                .global
                .branches
                .iter()
                .all(|entry| entry.function == function);
        let same = same
            && self
                .global
                .call_signatures
                .iter()
                .all(|entry| entry.caller == function)
            && self
                .global
                .arrays
                .iter()
                .all(|entry| entry.function() == function);
        same.then_some(function)
    }

    pub fn call_argument_types(&self, function: FunctionKey) -> Option<&[ObservedType]> {
        self.call_at(function)?.stable_types.as_deref()
    }

    pub fn call_at(&self, function: FunctionKey) -> Option<&CallFeedbackSnapshot> {
        self.global
            .calls
            .iter()
            .find(|entry| entry.function == function)
    }

    pub fn stable_return_at(&self, function: FunctionKey, pc: u32) -> Option<ObservedType> {
        self.global.entries.iter().find_map(|entry| {
            (entry.function == function
                && entry.pc == pc
                && entry.kind == FeedbackKind::Exit
                && entry.state == FeedbackState::Monomorphic
                && entry.observations.len() == 1)
                .then_some(entry.observations[0])
        })
    }

    pub fn binary_at(&self, function: FunctionKey, pc: u32) -> Option<&BinaryFeedbackSnapshot> {
        self.global
            .binaries
            .iter()
            .find(|entry| entry.function == function && entry.pc == pc)
    }

    pub fn conversion_at(
        &self,
        function: FunctionKey,
        pc: u32,
    ) -> Option<&ConversionFeedbackSnapshot> {
        self.global
            .conversions
            .iter()
            .find(|entry| entry.function == function && entry.pc == pc)
    }

    pub fn branch_at(&self, function: FunctionKey, pc: u32) -> Option<&BranchFeedbackSnapshot> {
        self.global
            .branches
            .iter()
            .find(|entry| entry.function == function && entry.pc == pc)
    }

    pub fn call_signature_at(
        &self,
        caller: FunctionKey,
        pc: u32,
    ) -> Option<&CallSignatureFeedbackSnapshot> {
        self.global
            .call_signatures
            .iter()
            .find(|entry| entry.caller == caller && entry.pc == pc)
    }

    pub fn call_specialization_at(
        &self,
        caller: FunctionKey,
        pc: u32,
    ) -> Option<CallSpecializationKey> {
        if self.epoch == 0 {
            return None;
        }
        let call = self.call_signature_at(caller, pc)?;
        if call.state != FeedbackState::Monomorphic || call.targets.len() != 1 {
            return None;
        }
        let arity = call.stable_arity?;
        if arity > MAX_SPECIALIZED_ARGUMENTS || call.arguments.len() != arity {
            return None;
        }
        let arguments = call
            .arguments
            .iter()
            .map(|observations| {
                (observations.len() == 1)
                    .then(|| observed_direct_argument_representation(observations[0]))
                    .flatten()
            })
            .collect::<Option<Vec<_>>>()?;
        if call.results.len() != 1 {
            return None;
        }
        Some(CallSpecializationKey {
            caller,
            callee: call.targets[0],
            arguments: arguments.into_boxed_slice(),
            result: observed_scalar_representation(call.results[0])?,
            feedback_epoch: self.epoch,
            callee_identity: call.callee_identity,
            callee_bytecode_identity: call.callee_bytecode_identity,
        })
    }

    /// Select a single exact target without requiring scalar arguments,
    /// monomorphic results, or a fixed observed arity. The bytecode operation
    /// and runtime admission checks supply the invocation contract separately.
    pub fn call_link_at(&self, caller: FunctionKey, pc: u32) -> Option<CallLinkStatus> {
        if self.epoch == 0 || caller.id == 0 || caller.generation == 0 {
            return None;
        }
        let call = self.call_signature_at(caller, pc)?;
        if call.target_state != FeedbackState::Monomorphic
            || call.targets.len() != 1
            || call.callee_identity == 0
            || call.callee_bytecode_identity == 0
        {
            return None;
        }
        let callee = call.targets[0];
        if callee.id == 0 || callee.generation == 0 {
            return None;
        }
        Some(CallLinkStatus {
            caller,
            pc,
            callee,
            callee_identity: call.callee_identity,
            callee_bytecode_identity: call.callee_bytecode_identity,
        })
    }

    pub fn call_specializations_for(
        &self,
        caller: FunctionKey,
    ) -> impl Iterator<Item = CallSpecializationKey> + '_ {
        self.global
            .call_signatures
            .iter()
            .filter(move |entry| entry.caller == caller)
            .filter_map(move |entry| self.call_specialization_at(caller, entry.pc))
    }

    /// Tier 2 may only consume feedback belonging to the exact function
    /// generation and frozen at a nonzero runtime epoch.
    pub fn has_stable_value_for(&self, function: FunctionKey) -> bool {
        self.epoch != 0
            && self.global.entries.iter().any(|entry| {
                entry.function == function
                    && entry.kind == FeedbackKind::Value
                    && entry.state == FeedbackState::Monomorphic
                    && entry.observations.len() == 1
            })
    }

    pub fn contains_stable_observation(
        &self,
        function: FunctionKey,
        pc: u32,
        observation: ObservedType,
    ) -> bool {
        self.global.entries.iter().any(|entry| {
            entry.function == function
                && entry.pc == pc
                && entry.kind == FeedbackKind::Exit
                && entry.state == FeedbackState::Monomorphic
                && entry.observations.as_ref() == [observation]
        })
    }

    pub fn stable_observation_at(&self, function: FunctionKey, pc: u32) -> Option<ObservedType> {
        self.global.entries.iter().find_map(|entry| {
            (entry.function == function
                && entry.pc == pc
                && entry.kind == FeedbackKind::Value
                && entry.state == FeedbackState::Monomorphic
                && entry.observations.len() == 1)
                .then_some(entry.observations[0])
        })
    }

    /// Builds the immutable, bounded key consumed by Tier 2 compilation.
    /// Arguments and returns may carry supported scalar representations,
    /// including Bool. Numeric arithmetic feedback must agree with a numeric
    /// result; each direct-entry lowerer still validates its complete body.
    pub fn bounded_specialization(
        &self,
        function: FunctionKey,
    ) -> Option<BoundedSpecializationSignature> {
        if self.epoch == 0 {
            return None;
        }
        let call = self.call_at(function)?;
        let arity = call.stable_arity?;
        if call.state != FeedbackState::Monomorphic || arity > MAX_SPECIALIZED_ARGUMENTS {
            return None;
        }
        let argument_types = call.stable_types.as_deref()?;
        if argument_types.len() != arity {
            return None;
        }

        let returns = self
            .global
            .entries
            .iter()
            .filter(|entry| entry.function == function && entry.kind == FeedbackKind::Exit);
        let mut return_type = None;
        for entry in returns {
            if entry.state != FeedbackState::Monomorphic || entry.observations.len() != 1 {
                return None;
            }
            let observed = entry.observations[0];
            if return_type
                .replace(observed)
                .is_some_and(|prior| prior != observed)
            {
                return None;
            }
        }
        let return_type = return_type?;
        let representation = match return_type {
            ObservedType::Int32 => FeedbackRepresentation::Int32,
            ObservedType::Float64 => FeedbackRepresentation::Float64,
            ObservedType::Bool => FeedbackRepresentation::Bool,
            _ => return None,
        };
        let arguments = argument_types
            .iter()
            .copied()
            .map(observed_representation)
            .collect::<Option<Vec<_>>>()?;

        let binaries = self
            .global
            .binaries
            .iter()
            .filter(|entry| entry.function == function);
        let mut matching_binary_count = 0;
        for binary in binaries {
            let expected = match representation {
                FeedbackRepresentation::Int32 => ObservedType::Int32,
                FeedbackRepresentation::Float64 => ObservedType::Float64,
                // Bool-result entries are still admitted by exact argument and
                // return feedback. Their lowerer validates every operation and
                // rejects an unsupported body before publication.
                FeedbackRepresentation::Bool => continue,
                FeedbackRepresentation::HeapRef => return None,
            };
            if binary.state == FeedbackState::Monomorphic
                && binary.lhs.as_ref() == [expected]
                && binary.rhs.as_ref() == [expected]
                && binary.result.as_ref() == [expected]
            {
                matching_binary_count += 1;
            }
        }
        if representation != FeedbackRepresentation::Bool && matching_binary_count == 0 {
            return None;
        }

        Some(BoundedSpecializationSignature {
            function,
            arguments: arguments.into_boxed_slice(),
            result: representation,
            feedback_epoch: self.epoch,
        })
    }
}

const fn observed_representation(observed: ObservedType) -> Option<FeedbackRepresentation> {
    match observed {
        ObservedType::Int32 => Some(FeedbackRepresentation::Int32),
        ObservedType::Float64 => Some(FeedbackRepresentation::Float64),
        ObservedType::Object => Some(FeedbackRepresentation::HeapRef),
        ObservedType::Bool => Some(FeedbackRepresentation::Bool),
        _ => None,
    }
}

// Target-only linked leaves accept guarded object payloads as borrowed HeapRef
// arguments. Results remain scalar and ownership-free.
const fn observed_direct_argument_representation(
    observed: ObservedType,
) -> Option<FeedbackRepresentation> {
    observed_representation(observed)
}

const fn observed_scalar_representation(observed: ObservedType) -> Option<FeedbackRepresentation> {
    match observed {
        ObservedType::Int32 => Some(FeedbackRepresentation::Int32),
        ObservedType::Float64 => Some(FeedbackRepresentation::Float64),
        ObservedType::Bool => Some(FeedbackRepresentation::Bool),
        _ => None,
    }
}

#[derive(Debug)]
pub struct FeedbackTable {
    // Frozen observations are immutable. Epoch and per-candidate shapes belong to the
    // snapshot wrapper, not to this shared allocation. Invalidate on change or retirement.
    frozen: OnceLock<Arc<GlobalFeedback>>,
    capacity: usize,
    diversity_limit: usize,
    dropped: u64,
    /// Incremented only when an observation changes a lattice entry. Stable,
    /// warmed-up feedback keeps this constant, which lets per-call runtime
    /// callbacks skip the snapshot/scan work that could not produce a
    /// different answer.
    version: u64,
    recent_types: [Option<(FeedbackKey, ObservedType, FeedbackState, u64)>; 4],
    recent_type_cursor: usize,
    last_call: Option<FunctionKey>,
    last_call_arguments: Vec<ObservedType>,
    entries: BTreeMap<FeedbackKey, Entry>,
    calls: BTreeMap<FunctionKey, CallFeedbackEntry>,
    binaries: BTreeMap<(FunctionKey, u32), BinaryFeedbackEntry>,
    conversions: BTreeMap<(FunctionKey, u32), ConversionFeedbackEntry>,
    branches: BTreeMap<(FunctionKey, u32), BranchFeedbackEntry>,
    call_signatures: BTreeMap<(FunctionKey, u32), CallSignatureFeedbackEntry>,
    arrays: super::ArrayFeedbackTable,
}

#[derive(Debug, Default)]
struct CallFeedbackEntry {
    slots: Vec<Entry>,
    arities: Vec<usize>,
    arity_megamorphic: bool,
}

#[derive(Debug)]
struct BinaryFeedbackEntry {
    lhs: Entry,
    rhs: Entry,
    result: Entry,
    flags: BinaryFeedbackFlags,
}

#[derive(Debug)]
struct ConversionFeedbackEntry {
    operand: Entry,
    result: Entry,
}

#[derive(Debug)]
struct BranchFeedbackEntry {
    condition_types: Entry,
    outcomes: u8,
}

#[derive(Debug)]
struct CallSignatureFeedbackEntry {
    targets: Vec<FunctionKey>,
    targets_megamorphic: bool,
    arities: Vec<usize>,
    arities_megamorphic: bool,
    arguments: Vec<Entry>,
    results: Entry,
    callee_identity: u64,
    callee_bytecode_identity: u64,
}

fn combined_state(entries: &[&Entry], additional_polymorphism: bool) -> FeedbackState {
    if entries.iter().any(|entry| entry.megamorphic) {
        FeedbackState::Megamorphic
    } else if additional_polymorphism || entries.iter().any(|entry| entry.observations.len() > 1) {
        FeedbackState::Polymorphic
    } else {
        FeedbackState::Monomorphic
    }
}

fn observe_bounded_distinct<T: Eq>(
    observations: &mut Vec<T>,
    megamorphic: &mut bool,
    observation: T,
    diversity_limit: usize,
) {
    if *megamorphic || observations.contains(&observation) {
        return;
    }
    if observations.len() >= diversity_limit {
        *megamorphic = true;
        observations.clear();
    } else {
        observations.push(observation);
    }
}

fn call_signature_state(entry: &CallSignatureFeedbackEntry) -> FeedbackState {
    if entry.targets_megamorphic
        || entry.arities_megamorphic
        || entry.results.megamorphic
        || entry.arguments.iter().any(|slot| slot.megamorphic)
    {
        FeedbackState::Megamorphic
    } else if entry.targets.len() != 1
        || entry.arities.len() != 1
        || entry.results.observations.len() != 1
        || entry
            .arguments
            .iter()
            .any(|slot| slot.observations.len() != 1)
    {
        FeedbackState::Polymorphic
    } else {
        FeedbackState::Monomorphic
    }
}

fn call_target_state(entry: &CallSignatureFeedbackEntry) -> FeedbackState {
    if entry.targets_megamorphic {
        FeedbackState::Megamorphic
    } else if entry.targets.len() == 1 {
        FeedbackState::Monomorphic
    } else {
        FeedbackState::Polymorphic
    }
}

/// Cheap structural fingerprint of one lattice cell: it changes exactly when
/// a new observation is admitted or the cell widens to megamorphic.
fn entry_shape(entry: &Entry) -> (usize, bool) {
    (entry.observations.len(), entry.megamorphic)
}

fn slots_shape(slots: &[Entry]) -> (usize, usize) {
    slots
        .iter()
        .fold((0, 0), |(observations, megamorphic), slot| {
            (
                observations + slot.observations.len(),
                megamorphic + usize::from(slot.megamorphic),
            )
        })
}

fn empty_entry(capacity: usize) -> Entry {
    Entry {
        observations: Vec::with_capacity(capacity),
        megamorphic: false,
    }
}

impl FeedbackTable {
    pub fn new(capacity: usize, diversity_limit: usize) -> Self {
        Self {
            frozen: OnceLock::new(),
            capacity,
            diversity_limit: diversity_limit.max(1),
            dropped: 0,
            version: 0,
            recent_types: [None; 4],
            recent_type_cursor: 0,
            last_call: None,
            last_call_arguments: Vec::new(),
            entries: BTreeMap::new(),
            calls: BTreeMap::new(),
            binaries: BTreeMap::new(),
            conversions: BTreeMap::new(),
            branches: BTreeMap::new(),
            call_signatures: BTreeMap::new(),
            arrays: super::ArrayFeedbackTable::new(capacity, diversity_limit),
        }
    }

    /// Monotonic change counter; see the field documentation.
    pub const fn version(&self) -> u64 {
        self.version
    }

    /// Drops every observation about `function` once its bytecode
    /// generation is freed. Only live generations can use feedback, and the
    /// capacity bound would otherwise fill up with retired functions (for
    /// example one-shot eval scripts), after which no new function could
    /// record the return types that call links and native calls require.
    pub fn forget_function(&mut self, function: FunctionKey) {
        // The version is left unchanged: a freed generation can never be
        // compiled again, so no live lattice entry changes and artifacts in
        // flight for other functions keep their feedback epoch.
        self.frozen.take();
        self.entries.retain(|key, _| key.function != function);
        self.calls.remove(&function);
        self.binaries.retain(|(key, _), _| *key != function);
        self.conversions.retain(|(key, _), _| *key != function);
        self.branches.retain(|(key, _), _| *key != function);
        self.call_signatures.retain(|(key, _), _| *key != function);
        self.arrays.forget_function(function);
        if self.last_call == Some(function) {
            self.last_call = None;
            self.last_call_arguments.clear();
        }
        self.recent_types = [None; 4];
    }

    pub fn observe_array(
        &mut self,
        function: FunctionKey,
        pc: u32,
        access: super::ArrayAccess,
        mode: super::ArrayMode,
        hazards: super::ArrayHazards,
    ) -> FeedbackState {
        let version = self.arrays.version();
        let state = self.arrays.observe(function, pc, access, mode, hazards);
        if self.arrays.version() != version {
            self.frozen.take();
            self.version = self.version.saturating_add(1);
        }
        state
    }

    pub fn observe_array_raw(
        &mut self,
        function: FunctionKey,
        pc: u32,
        mode: u32,
        flags: u32,
    ) -> Option<FeedbackState> {
        let version = self.arrays.version();
        let state = self.arrays.observe_raw(function, pc, mode, flags)?;
        if self.arrays.version() != version {
            self.frozen.take();
            self.version = self.version.saturating_add(1);
        }
        Some(state)
    }

    pub fn observe_call(&mut self, function: FunctionKey, arguments: &[ObservedType]) {
        // Call feedback is a monotonic lattice, not an execution counter. An
        // identical event has already contributed every argument and its arity.
        if self.last_call == Some(function) && self.last_call_arguments == arguments {
            return;
        }
        let (call, is_new) = match self.calls.entry(function) {
            MapEntry::Occupied(entry) => (entry.into_mut(), false),
            MapEntry::Vacant(entry) => (entry.insert(CallFeedbackEntry::default()), true),
        };
        let before = (
            call.arities.len(),
            call.arity_megamorphic,
            call.slots.len(),
            slots_shape(&call.slots),
        );
        if !call.arity_megamorphic && !call.arities.contains(&arguments.len()) {
            if call.arities.len() >= self.diversity_limit {
                call.arity_megamorphic = true;
                call.arities.clear();
            } else {
                call.arities.push(arguments.len());
            }
        }
        if call.slots.len() < arguments.len() {
            call.slots
                .resize_with(arguments.len(), || empty_entry(self.diversity_limit));
        }
        for (slot, observation) in call.slots.iter_mut().zip(arguments.iter().copied()) {
            slot.observe(observation, self.diversity_limit);
        }
        let after = (
            call.arities.len(),
            call.arity_megamorphic,
            call.slots.len(),
            slots_shape(&call.slots),
        );
        if is_new || before != after {
            self.frozen.take();
            self.version = self.version.wrapping_add(1);
        }
        self.last_call = Some(function);
        self.last_call_arguments.clear();
        self.last_call_arguments.extend_from_slice(arguments);
    }

    pub fn observe_return(&mut self, function: FunctionKey, pc: u32, result: ObservedType) {
        self.observe_type(function, pc, FeedbackKind::Exit, result);
    }

    pub fn observe_binary(
        &mut self,
        function: FunctionKey,
        pc: u32,
        lhs: ObservedType,
        rhs: ObservedType,
        result: ObservedType,
        flags: BinaryFeedbackFlags,
    ) -> FeedbackState {
        let entry = self
            .binaries
            .entry((function, pc))
            .or_insert_with(|| BinaryFeedbackEntry {
                lhs: empty_entry(self.diversity_limit),
                rhs: empty_entry(self.diversity_limit),
                result: empty_entry(self.diversity_limit),
                flags: BinaryFeedbackFlags::NONE,
            });
        let before = (
            entry_shape(&entry.lhs),
            entry_shape(&entry.rhs),
            entry_shape(&entry.result),
            entry.flags,
        );
        let states = [
            entry.lhs.observe(lhs, self.diversity_limit),
            entry.rhs.observe(rhs, self.diversity_limit),
            entry.result.observe(result, self.diversity_limit),
        ];
        entry.flags |= flags;
        let after = (
            entry_shape(&entry.lhs),
            entry_shape(&entry.rhs),
            entry_shape(&entry.result),
            entry.flags,
        );
        if before != after {
            self.frozen.take();
            self.version = self.version.wrapping_add(1);
        }
        if states.contains(&FeedbackState::Megamorphic) {
            FeedbackState::Megamorphic
        } else if states.contains(&FeedbackState::Polymorphic) {
            FeedbackState::Polymorphic
        } else {
            FeedbackState::Monomorphic
        }
    }

    pub fn observe_conversion(
        &mut self,
        function: FunctionKey,
        pc: u32,
        operand: ObservedType,
        result: ObservedType,
    ) -> FeedbackState {
        let entry =
            self.conversions
                .entry((function, pc))
                .or_insert_with(|| ConversionFeedbackEntry {
                    operand: empty_entry(self.diversity_limit),
                    result: empty_entry(self.diversity_limit),
                });
        let before = (entry_shape(&entry.operand), entry_shape(&entry.result));
        entry.operand.observe(operand, self.diversity_limit);
        entry.result.observe(result, self.diversity_limit);
        if before != (entry_shape(&entry.operand), entry_shape(&entry.result)) {
            self.frozen.take();
            self.version = self.version.wrapping_add(1);
        }
        combined_state(&[&entry.operand, &entry.result], false)
    }

    pub fn observe_branch(
        &mut self,
        function: FunctionKey,
        pc: u32,
        condition_type: ObservedType,
        taken: bool,
    ) -> FeedbackState {
        let entry = self
            .branches
            .entry((function, pc))
            .or_insert_with(|| BranchFeedbackEntry {
                condition_types: empty_entry(self.diversity_limit),
                outcomes: 0,
            });
        let before = (entry_shape(&entry.condition_types), entry.outcomes);
        entry
            .condition_types
            .observe(condition_type, self.diversity_limit);
        entry.outcomes |= if taken { 1 } else { 2 };
        if before != (entry_shape(&entry.condition_types), entry.outcomes) {
            self.frozen.take();
            self.version = self.version.wrapping_add(1);
        }
        combined_state(&[&entry.condition_types], entry.outcomes == 3)
    }

    pub fn observe_call_signature(
        &mut self,
        caller: FunctionKey,
        pc: u32,
        callee: FunctionKey,
        arguments: &[ObservedType],
        result: ObservedType,
    ) -> FeedbackState {
        self.observe_call_signature_with_identity(caller, pc, callee, 0, 0, arguments, result)
    }

    #[allow(clippy::too_many_arguments)] // The feedback record's fields are sampled independently at each call site.
    pub fn observe_call_signature_with_identity(
        &mut self,
        caller: FunctionKey,
        pc: u32,
        callee: FunctionKey,
        callee_identity: u64,
        callee_bytecode_identity: u64,
        arguments: &[ObservedType],
        result: ObservedType,
    ) -> FeedbackState {
        let entry = self.call_signatures.entry((caller, pc)).or_insert_with(|| {
            CallSignatureFeedbackEntry {
                targets: Vec::with_capacity(self.diversity_limit),
                targets_megamorphic: false,
                arities: Vec::with_capacity(self.diversity_limit),
                arities_megamorphic: false,
                arguments: Vec::new(),
                results: empty_entry(self.diversity_limit),
                callee_identity,
                callee_bytecode_identity,
            }
        });
        let before = (
            entry.targets.len(),
            entry.targets_megamorphic,
            entry.arities.len(),
            entry.arities_megamorphic,
            entry.arguments.len(),
            slots_shape(&entry.arguments),
            entry_shape(&entry.results),
            entry.callee_identity,
            entry.callee_bytecode_identity,
        );
        if entry.callee_identity != callee_identity {
            entry.targets_megamorphic = true;
            entry.callee_identity = 0;
        }
        if entry.callee_bytecode_identity != callee_bytecode_identity {
            entry.targets_megamorphic = true;
            entry.callee_bytecode_identity = 0;
        }
        observe_bounded_distinct(
            &mut entry.targets,
            &mut entry.targets_megamorphic,
            callee,
            self.diversity_limit,
        );
        observe_bounded_distinct(
            &mut entry.arities,
            &mut entry.arities_megamorphic,
            arguments.len(),
            self.diversity_limit,
        );
        if entry.arguments.len() < arguments.len() {
            entry
                .arguments
                .resize_with(arguments.len(), || empty_entry(self.diversity_limit));
        }
        for (slot, observed) in entry.arguments.iter_mut().zip(arguments.iter().copied()) {
            slot.observe(observed, self.diversity_limit);
        }
        entry.results.observe(result, self.diversity_limit);
        let after = (
            entry.targets.len(),
            entry.targets_megamorphic,
            entry.arities.len(),
            entry.arities_megamorphic,
            entry.arguments.len(),
            slots_shape(&entry.arguments),
            entry_shape(&entry.results),
            entry.callee_identity,
            entry.callee_bytecode_identity,
        );
        if before != after {
            self.frozen.take();
            self.version = self.version.wrapping_add(1);
        }
        call_signature_state(entry)
    }

    pub fn observe_type(
        &mut self,
        function: FunctionKey,
        pc: u32,
        kind: FeedbackKind,
        observation: ObservedType,
    ) -> FeedbackState {
        let key = FeedbackKey { function, pc, kind };
        // A cached result is valid only at the same lattice version: a later
        // observation can widen the state returned for an older known type.
        if let Some((_, _, state, _)) =
            self.recent_types
                .iter()
                .flatten()
                .find(|(cached, ty, _, version)| {
                    *cached == key && *ty == observation && *version == self.version
                })
        {
            return *state;
        }
        let at_capacity = self.entries.len() >= self.capacity;
        let (entry, before) = match self.entries.entry(key) {
            MapEntry::Occupied(entry) => {
                let before = entry_shape(entry.get());
                (entry.into_mut(), Some(before))
            }
            MapEntry::Vacant(entry) => {
                if at_capacity {
                    self.dropped = self.dropped.saturating_add(1);
                    return FeedbackState::Megamorphic;
                }
                (entry.insert(empty_entry(self.diversity_limit)), None)
            }
        };
        let state = entry.observe(observation, self.diversity_limit);
        if before != Some(entry_shape(entry)) {
            self.frozen.take();
            self.version = self.version.wrapping_add(1);
        }
        // Capacity-rejected observations return above and are never cached:
        // each rejected event must still increment the dropped counter.
        self.recent_types[self.recent_type_cursor] = Some((key, observation, state, self.version));
        self.recent_type_cursor = (self.recent_type_cursor + 1) % self.recent_types.len();
        state
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub const fn dropped_observations(&self) -> u64 {
        self.dropped
            .saturating_add(self.arrays.dropped_observations())
    }

    pub fn snapshot(&self, epoch: u64) -> FeedbackSnapshot {
        FeedbackSnapshot {
            epoch,
            global: self.frozen.get_or_init(|| Arc::new(self.freeze())).clone(),
            properties: Box::new([]),
        }
    }

    fn freeze(&self) -> GlobalFeedback {
        let entries = self
            .entries
            .iter()
            .map(|(key, entry)| FeedbackSnapshotEntry {
                function: key.function,
                pc: key.pc,
                kind: key.kind,
                state: if entry.megamorphic {
                    FeedbackState::Megamorphic
                } else if entry.observations.len() == 1 {
                    FeedbackState::Monomorphic
                } else {
                    FeedbackState::Polymorphic
                },
                observations: entry.observations.clone().into_boxed_slice(),
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let calls = self
            .calls
            .iter()
            .map(|(function, call)| {
                let slots = &call.slots;
                let state = if call.arity_megamorphic || slots.iter().any(|slot| slot.megamorphic) {
                    FeedbackState::Megamorphic
                } else if call.arities.len() != 1
                    || slots.iter().any(|slot| slot.observations.len() != 1)
                {
                    FeedbackState::Polymorphic
                } else {
                    FeedbackState::Monomorphic
                };
                let stable_types = (state == FeedbackState::Monomorphic).then(|| {
                    slots
                        .iter()
                        .map(|slot| slot.observations[0])
                        .collect::<Vec<_>>()
                        .into_boxed_slice()
                });
                CallFeedbackSnapshot {
                    function: *function,
                    state,
                    arguments: slots
                        .iter()
                        .map(|slot| slot.observations.clone().into_boxed_slice())
                        .collect::<Vec<_>>()
                        .into_boxed_slice(),
                    stable_types,
                    stable_arity: if call.arities.len() == 1 {
                        Some(call.arities[0])
                    } else {
                        None
                    },
                }
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let binaries = self
            .binaries
            .iter()
            .map(|((function, pc), entry)| BinaryFeedbackSnapshot {
                function: *function,
                pc: *pc,
                state: combined_state(&[&entry.lhs, &entry.rhs, &entry.result], false),
                lhs: entry.lhs.observations.clone().into_boxed_slice(),
                rhs: entry.rhs.observations.clone().into_boxed_slice(),
                result: entry.result.observations.clone().into_boxed_slice(),
                flags: entry.flags,
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let conversions = self
            .conversions
            .iter()
            .map(|((function, pc), entry)| ConversionFeedbackSnapshot {
                function: *function,
                pc: *pc,
                state: combined_state(&[&entry.operand, &entry.result], false),
                operand: entry.operand.observations.clone().into_boxed_slice(),
                result: entry.result.observations.clone().into_boxed_slice(),
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let branches = self
            .branches
            .iter()
            .map(|((function, pc), entry)| BranchFeedbackSnapshot {
                function: *function,
                pc: *pc,
                state: combined_state(&[&entry.condition_types], entry.outcomes == 3),
                condition_types: entry
                    .condition_types
                    .observations
                    .clone()
                    .into_boxed_slice(),
                outcomes: entry.outcomes,
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let call_signatures = self
            .call_signatures
            .iter()
            .map(|((caller, pc), entry)| CallSignatureFeedbackSnapshot {
                caller: *caller,
                pc: *pc,
                state: call_signature_state(entry),
                target_state: call_target_state(entry),
                targets: entry.targets.clone().into_boxed_slice(),
                stable_arity: if entry.arities.len() == 1 {
                    Some(entry.arities[0])
                } else {
                    None
                },
                arguments: entry
                    .arguments
                    .iter()
                    .map(|slot| slot.observations.clone().into_boxed_slice())
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
                results: entry.results.observations.clone().into_boxed_slice(),
                callee_identity: entry.callee_identity,
                callee_bytecode_identity: entry.callee_bytecode_identity,
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        GlobalFeedback {
            entries,
            calls,
            binaries,
            conversions,
            branches,
            call_signatures,
            arrays: self.arrays.snapshot(),
        }
    }
}

#[cfg(test)]
mod call_link_tests {
    use super::*;

    const CALLER: FunctionKey = FunctionKey::new(31, 1);
    const CALLEE: FunctionKey = FunctionKey::new(32, 2);
    const PC: u32 = 17;

    fn observe(table: &mut FeedbackTable, arguments: &[ObservedType], result: ObservedType) {
        table.observe_call_signature_with_identity(
            CALLER, PC, CALLEE, 0x1000, 0x2000, arguments, result,
        );
    }

    #[test]
    fn snapshots_share_only_unchanged_observations_and_keep_the_requested_epoch() {
        let mut table = FeedbackTable::new(16, 2);
        table.observe_return(CALLER, PC, ObservedType::Int32);
        let first = table.snapshot(1);
        table.observe_return(CALLER, PC, ObservedType::Int32);
        let next = table.snapshot(2);
        assert_eq!(next.epoch(), 2);
        assert!(Arc::ptr_eq(&first.global, &next.global));
        table.observe_return(CALLER, PC, ObservedType::Bool);
        let wide = table.snapshot(2);
        assert!(!Arc::ptr_eq(&first.global, &wide.global));
        assert_eq!(first.entries()[0].state(), FeedbackState::Monomorphic);
        assert_eq!(wide.entries()[0].state(), FeedbackState::Polymorphic);
        let version = table.version();
        table.forget_function(CALLER);
        assert_eq!(version, table.version()); // retirement doesn't advance the lattice
        assert!(table.snapshot(2).entries().is_empty());
        assert_eq!(wide.entries().len(), 1); // a worker can keep its old snapshot
    }

    #[test]
    fn retired_functions_release_their_feedback_capacity() {
        let mut table = FeedbackTable::new(2, 2);
        let retired = FunctionKey::new(10, 10);
        let live = FunctionKey::new(11, 11);
        table.observe_return(retired, 1, ObservedType::Int32);
        table.observe_return(retired, 2, ObservedType::Int32);
        assert_eq!(
            table.observe_type(live, 1, FeedbackKind::Exit, ObservedType::Int32),
            FeedbackState::Megamorphic
        );
        table.forget_function(retired);
        assert_eq!(
            table.observe_type(live, 1, FeedbackKind::Exit, ObservedType::Int32),
            FeedbackState::Monomorphic
        );
        let snapshot = table.snapshot(1);
        assert!(snapshot
            .entries()
            .iter()
            .all(|entry| entry.function() == live));
    }

    #[test]
    fn object_argument_links_the_target_with_a_guarded_heapref_specialization() {
        let mut table = FeedbackTable::new(16, 2);
        observe(
            &mut table,
            &[
                ObservedType::Int32,
                ObservedType::Bool,
                ObservedType::Object,
            ],
            ObservedType::Int32,
        );
        let feedback = table.snapshot(1);
        let link = feedback
            .call_link_at(CALLER, PC)
            .expect("object argument must permit target linking");
        assert_eq!(link.caller(), CALLER);
        assert_eq!(link.pc(), PC);
        assert_eq!(link.callee(), CALLEE);
        assert_eq!(link.callee_identity(), 0x1000);
        assert_eq!(link.callee_bytecode_identity(), 0x2000);
        let direct = feedback
            .call_specialization_at(CALLER, PC)
            .expect("guarded object argument direct specialization");
        assert_eq!(
            direct.arguments(),
            &[
                FeedbackRepresentation::Int32,
                FeedbackRepresentation::Bool,
                FeedbackRepresentation::HeapRef,
            ]
        );
    }

    #[test]
    fn monomorphic_bool_result_is_a_bounded_direct_call_specialization() {
        let mut table = FeedbackTable::new(16, 2);
        observe(
            &mut table,
            &[ObservedType::Object, ObservedType::Bool],
            ObservedType::Bool,
        );
        let direct = table
            .snapshot(1)
            .call_specialization_at(CALLER, PC)
            .expect("guarded Bool result specialization");
        assert_eq!(
            direct.arguments(),
            &[
                FeedbackRepresentation::HeapRef,
                FeedbackRepresentation::Bool
            ]
        );
        assert_eq!(direct.result(), FeedbackRepresentation::Bool);
    }

    #[test]
    fn argument_result_and_arity_widening_preserve_one_target() {
        let mut table = FeedbackTable::new(16, 2);
        observe(&mut table, &[ObservedType::Int32], ObservedType::Int32);
        let initial = table.snapshot(1).call_link_at(CALLER, PC).unwrap();
        for (arguments, result) in [
            (&[ObservedType::Object][..], ObservedType::Object),
            (
                &[ObservedType::String, ObservedType::Bool][..],
                ObservedType::String,
            ),
            (&[][..], ObservedType::Undefined),
        ] {
            observe(&mut table, arguments, result);
        }
        let feedback = table.snapshot(2);
        let call = feedback.call_signature_at(CALLER, PC).unwrap();
        assert_eq!(call.state(), FeedbackState::Megamorphic);
        assert_eq!(call.target_state(), FeedbackState::Monomorphic);
        assert_eq!(feedback.call_link_at(CALLER, PC), Some(initial));
        assert!(feedback.call_specialization_at(CALLER, PC).is_none());
    }

    #[test]
    fn distinct_closures_sharing_bytecode_permanently_invalidate_the_exact_link() {
        let mut table = FeedbackTable::new(16, 2);
        observe(&mut table, &[ObservedType::Int32], ObservedType::Int32);
        assert!(table.snapshot(1).call_link_at(CALLER, PC).is_some());
        table.observe_call_signature_with_identity(
            CALLER,
            PC,
            CALLEE,
            0x3000,
            0x2000,
            &[ObservedType::Int32],
            ObservedType::Int32,
        );
        observe(&mut table, &[ObservedType::Int32], ObservedType::Int32);
        let feedback = table.snapshot(2);
        assert_eq!(
            feedback
                .call_signature_at(CALLER, PC)
                .unwrap()
                .target_state(),
            FeedbackState::Megamorphic
        );
        assert!(feedback.call_link_at(CALLER, PC).is_none());
    }

    #[test]
    fn target_diversity_budget_remains_bounded_and_absorbing() {
        let mut table = FeedbackTable::new(16, 2);
        for id in 32..35 {
            table.observe_call_signature_with_identity(
                CALLER,
                PC,
                FunctionKey::new(id, 2),
                0x1000,
                0x2000,
                &[ObservedType::Int32],
                ObservedType::Int32,
            );
        }
        observe(&mut table, &[ObservedType::Int32], ObservedType::Int32);
        let feedback = table.snapshot(2);
        let call = feedback.call_signature_at(CALLER, PC).unwrap();
        assert!(call.targets().is_empty());
        assert_eq!(call.target_state(), FeedbackState::Megamorphic);
        assert!(feedback.call_link_at(CALLER, PC).is_none());
    }

    #[test]
    fn missing_identity_and_invalid_epoch_do_not_produce_links() {
        let mut table = FeedbackTable::new(16, 2);
        table.observe_call_signature(
            CALLER,
            PC,
            CALLEE,
            &[ObservedType::Int32],
            ObservedType::Int32,
        );
        assert!(table.snapshot(1).call_link_at(CALLER, PC).is_none());
        let mut table = FeedbackTable::new(16, 2);
        observe(&mut table, &[ObservedType::Int32], ObservedType::Int32);
        assert!(table.snapshot(0).call_link_at(CALLER, PC).is_none());
        assert!(table.snapshot(1).call_link_at(CALLER, PC + 1).is_none());
    }
}
