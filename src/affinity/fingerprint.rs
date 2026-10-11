//! Typed canonical history projection with incremental, content-free field hashing.

use super::{Anchor, Digest, Evidence, HistoryPrefix, Projection, RetryFingerprint};
use hmac::{Hmac, KeyInit as _, Mac as _};
use serde::{
    Deserialize, Serialize, Serializer,
    ser::{SerializeMap as _, SerializeSeq as _},
};
use serde_json::Value;
use sha2::Sha256;
use std::{
    collections::{BTreeMap, HashMap},
    io::{self, Write},
    num::NonZeroU64,
    sync::LazyLock,
};

#[cfg(test)]
mod tests;

/// One process-local key, independent of sealing keys and stable through configuration reload.
static PROCESS_KEY: LazyLock<Key> = LazyLock::new(|| Key::from_bytes(rand::random()));

/// Initializes the process key at server startup, never reading or writing persistent key data.
pub(crate) fn initialize_key() {
    LazyLock::force(&PROCESS_KEY);
}

/// Pre-keyed HMAC state, retaining no source content and intentionally lacking debug output.
#[derive(Clone)]
struct Key(Hmac<Sha256>);

impl Key {
    /// Creates a pre-keyed state from the fixed process-key domain.
    ///
    /// # Panics
    ///
    /// Panics if HMAC rejects a 32-byte key, an impossible library contract violation.
    fn from_bytes(bytes: [u8; 32]) -> Self {
        #[expect(clippy::expect_used, reason = "HMAC accepts keys of every byte length")]
        let authentication =
            Hmac::new_from_slice(&bytes).expect("the 32-byte affinity key is valid");
        Self(authentication)
    }

    /// Starts a domain-separated field, permitting later incremental updates of exact bytes.
    fn field(&self, domain: FieldDomain) -> Hmac<Sha256> {
        let mut authentication = self.0.clone();
        authentication.update(b"janusgate.affinity.field.v1\0");
        authentication.update(domain.bytes());
        authentication.update(b"\0");
        authentication
    }

    /// Replaces accepted variable field content with its exact domain-separated identity.
    fn bytes(&self, domain: FieldDomain, bytes: &[u8]) -> Digest {
        let mut authentication = self.field(domain);
        authentication.update(bytes);
        finish(authentication)
    }

    /// Hashes only foreign JSON, with recursive object sorting and normal scalar serialization.
    ///
    /// # Panics
    ///
    /// Panics if an accepted JSON value cannot serialize to the infallible hashing writer.
    fn json(&self, domain: FieldDomain, value: &Value) -> Digest {
        self.canonical_json(domain, &CanonicalJson(value))
    }

    /// Hashes a codec-owned view that already serializes foreign members in canonical order.
    ///
    /// # Panics
    ///
    /// Panics if the typed accepted view cannot serialize to the infallible hashing writer.
    fn canonical_json(&self, domain: FieldDomain, value: &impl Serialize) -> Digest {
        let mut authentication = self.field(domain);
        #[expect(
            clippy::expect_used,
            reason = "JSON values serialize to an infallible hash sink"
        )]
        serde_json::to_writer(HashWriter(&mut authentication), value)
            .expect("accepted foreign JSON serializes");
        finish(authentication)
    }

    /// Hashes a borrowed canonical JSON string without constructing another carrier buffer.
    ///
    /// # Panics
    ///
    /// Panics if a string cannot serialize to the infallible hashing writer.
    fn json_string(&self, domain: FieldDomain, value: &str) -> Digest {
        let mut authentication = self.field(domain);
        #[expect(
            clippy::expect_used,
            reason = "strings serialize to an infallible hash sink"
        )]
        serde_json::to_writer(HashWriter(&mut authentication), value)
            .expect("the accepted carrier string serializes");
        finish(authentication)
    }

    /// The empty history chain, permitting a complete empty-history retry without a fake unit.
    fn root(&self) -> Digest {
        let mut authentication = self.0.clone();
        authentication.update(b"janusgate.affinity.history.v1");
        finish(authentication)
    }

    /// Frames one typed canonical unit onto its preceding chain digest.
    ///
    /// # Panics
    ///
    /// Panics if fixed-shape digest metadata fails to serialize or exceeds the u64 length domain.
    fn unit(&self, previous: Digest, unit: &Unit) -> Digest {
        #[expect(
            clippy::expect_used,
            reason = "typed canonical digest metadata always serializes"
        )]
        let bytes = serde_json::to_vec(unit).expect("the canonical unit serializes");
        #[expect(
            clippy::expect_used,
            reason = "a resident unit cannot exceed u64 addressable bytes"
        )]
        let length = u64::try_from(bytes.len()).expect("canonical unit length fits u64");
        let mut authentication = self.0.clone();
        authentication.update(b"unit");
        authentication.update(&previous.0);
        authentication.update(&length.to_be_bytes());
        authentication.update(&bytes);
        finish(authentication)
    }
}

/// Fixed ASCII variable-field domains; client strings never become domain separators.
#[derive(Clone, Copy)]
enum FieldDomain {
    /// Concatenated exact UTF-8 text bytes.
    Text,
    /// Inline image MIME type.
    MimeType,
    /// Exact parser-accepted base64 image bytes.
    ImageData,
    /// Exact remote URL, without fetching or rewriting.
    Url,
    /// Function name.
    Name,
    /// Real wire identity, or request-local relationship lookup only for a synthetic identity.
    CallId,
    /// Canonical foreign argument JSON.
    Arguments,
    /// Accepted result content.
    Results,
    /// Replayable signature bytes.
    Signatures,
    /// Exact canonical foreign reasoning/state payload.
    OpaqueState,
}

impl FieldDomain {
    /// Stable field-domain vocabulary of the version-one projection contract.
    fn bytes(self) -> &'static [u8] {
        match self {
            Self::Text => b"text",
            Self::MimeType => b"mime_type",
            Self::ImageData => b"image_data",
            Self::Url => b"urls",
            Self::Name => b"names",
            Self::CallId => b"call_ids",
            Self::Arguments => b"arguments",
            Self::Results => b"results",
            Self::Signatures => b"signatures",
            Self::OpaqueState => b"opaque_state",
        }
    }
}

/// Consumes a field/chain state into its fixed-size fingerprint.
fn finish(authentication: Hmac<Sha256>) -> Digest {
    Digest(authentication.finalize().into_bytes().into())
}

/// The known codec payload positions whose text fields become independent digests.
#[derive(Clone, Copy)]
enum ReasoningJsonMode {
    /// Foreign fragments retain their complete canonical representation.
    Foreign,
    /// Messages thinking has one text field and a separately hashed signature.
    Messages,
    /// Interactions thought has ordered summary content and a separate signature.
    Interactions,
    /// One ordered Interactions summary block.
    Summary,
}

impl ReasoningJsonMode {
    /// Recognizes only the replayable codec-owned payload family; unknown fragments stay foreign.
    fn root(family: ReplayFamily, value: &Value) -> Self {
        match (family, value.get("type").and_then(Value::as_str)) {
            (ReplayFamily::Messages, Some("thinking")) => Self::Messages,
            (ReplayFamily::Interactions, Some("thought")) => Self::Interactions,
            _ => Self::Foreign,
        }
    }
}

/// Borrowed foreign payload normalization, replacing only known reasoning text fields.
struct ReasoningJson<'a> {
    /// Process-local field authentication.
    key: &'a Key,
    /// Existing accepted replay fragment; no text is copied.
    value: &'a Value,
    /// Codec-owned position; annotations and other unknown fragments remain canonical JSON.
    mode: ReasoningJsonMode,
}

impl Serialize for ReasoningJson<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let Value::Object(object) = self.value else {
            return CanonicalJson(self.value).serialize(serializer);
        };
        if matches!(self.mode, ReasoningJsonMode::Foreign) {
            return CanonicalJson(self.value).serialize(serializer);
        }
        let is_text_summary = matches!(self.mode, ReasoningJsonMode::Summary)
            && object.get("type").and_then(Value::as_str) == Some("text");
        let mut entries: Vec<_> = object.iter().collect();
        entries.sort_unstable_by_key(|(key, _)| *key);
        let mut map = serializer.serialize_map(None)?;
        for (name, value) in entries {
            if name == "signature"
                && matches!(
                    self.mode,
                    ReasoningJsonMode::Messages | ReasoningJsonMode::Interactions
                )
            {
                continue;
            }
            if ((name == "thinking" && matches!(self.mode, ReasoningJsonMode::Messages))
                || (name == "text" && is_text_summary))
                && let Some(text) = value.as_str()
            {
                map.serialize_entry(name, &self.key.bytes(FieldDomain::Text, text.as_bytes()))?;
            } else if name == "summary"
                && matches!(self.mode, ReasoningJsonMode::Interactions)
                && let Some(summary) = value.as_array()
            {
                let blocks: Vec<_> = summary
                    .iter()
                    .map(|value| Self {
                        key: self.key,
                        value,
                        mode: ReasoningJsonMode::Summary,
                    })
                    .collect();
                map.serialize_entry(name, &blocks)?;
            } else {
                map.serialize_entry(name, &CanonicalJson(value))?;
            }
        }
        map.end()
    }
}

/// Canonical serializer for foreign JSON fragments whose shape belongs to the client/provider.
struct CanonicalJson<'a>(&'a Value);

impl Serialize for CanonicalJson<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Value::Object(object) => {
                let mut entries: Vec<_> = object.iter().collect();
                entries.sort_unstable_by_key(|(key, _)| *key);
                let mut map = serializer.serialize_map(Some(entries.len()))?;
                for (key, value) in entries {
                    map.serialize_entry(key, &CanonicalJson(value))?;
                }
                map.end()
            }
            Value::Array(array) => {
                let mut sequence = serializer.serialize_seq(Some(array.len()))?;
                for value in array {
                    sequence.serialize_element(&CanonicalJson(value))?;
                }
                sequence.end()
            }
            scalar => scalar.serialize(serializer),
        }
    }
}

/// Canonical foreign JSON borrowing every existing string, including object member names.
/// Object ordering is lexical; array ordering and normal scalar representations are preserved.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum BorrowedJson<'a> {
    /// JSON null.
    Null,
    /// JSON boolean.
    Boolean(bool),
    /// Normal JSON number representation.
    Number(serde_json::Number),
    /// Existing foreign text.
    Text(#[serde(borrow)] &'a str),
    /// Ordered foreign fragments.
    Array(#[serde(borrow)] Vec<Self>),
    /// Foreign member references, recursively sorted without copying their text.
    Object(#[serde(borrow)] BTreeMap<&'a str, Self>),
}

/// Serialization sink feeding only a digest, without assembling another content buffer.
struct HashWriter<'a>(&'a mut Hmac<Sha256>);

impl Write for HashWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Conversation role; instructions have no canonical role or projection entry point.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Role {
    /// Client conversation turn.
    User,
    /// Replayable outward model output.
    Assistant,
    /// Tool results delivered to the model.
    Tool,
}

/// Replay family of an opaque signature/state payload.
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReplayFamily {
    /// OpenAI Responses state and reasoning.
    Responses,
    /// Anthropic Messages state and reasoning.
    Messages,
    /// Gemini GenerateContent replayable signatures.
    GenerateContent,
    /// Gemini Interactions thought payloads.
    Interactions,
}

/// Position-preserving state identity, excluding triggers and no-op controls.
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StateKind {
    /// Provider-native state replayed by its codec.
    Native,
    /// Gateway carrier still in its client-visible sealed representation.
    Gateway,
}

/// Completed codec-normalized state fields awaiting their actual outward history position.
#[derive(Clone, Copy)]
pub(crate) struct PreparedState {
    /// Actual replay protocol family.
    family: ReplayFamily,
    /// Native state or authenticated gateway token.
    kind: StateKind,
    /// Canonical field digest; no state text survives preparation.
    payload: Digest,
}

/// Actual source identity, separated from the inference parser's resolved internal reference.
#[derive(Clone, Copy)]
pub(crate) enum Identity<'a> {
    /// The accepted client wire supplied this non-synthetic identity.
    Wire(&'a str),
    /// The codec resolved this id-less/legacy reference; it never enters a canonical unit.
    Synthetic(&'a str),
}

impl<'a> Identity<'a> {
    /// Internal lookup bytes only; synthetic bytes never become projected part metadata.
    fn resolved(self) -> &'a str {
        match self {
            Self::Wire(value) | Self::Synthetic(value) => value,
        }
    }
}

/// Accepted tool result semantics, retaining the distinction between text and foreign JSON.
pub(crate) enum ResultContent<'a> {
    /// Exact accepted result text.
    Text(&'a str),
    /// Canonical accepted JSON result or content-block array.
    Json(&'a Value),
}

/// Fixed-shape canonical relationship identity, without source call IDs in the relational path.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
enum CallIdentity {
    /// Exact real wire ID digest.
    Exact(Digest),
    /// Zero-based call ordinal in canonical history order.
    Relational(u64),
}

/// Fixed-shape accepted result content identity.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "digest", rename_all = "snake_case")]
enum Content {
    /// Exact text, preserving byte and scalar-kind distinctions.
    Text(Digest),
    /// Canonical foreign JSON.
    Json(Digest),
}

/// Digest-only canonical parts. Their order is always the actual codec replay order.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Part {
    /// One adjacent exact text run.
    Text {
        /// Domain-separated concatenated text identity.
        digest: Digest,
    },
    /// Accepted inline image metadata and payload.
    InlineImage {
        /// Accepted MIME type identity.
        mime_type: Digest,
        /// Accepted base64 payload identity.
        data: Digest,
    },
    /// Accepted remote image URL.
    UrlImage {
        /// Exact accepted URL identity.
        url: Digest,
    },
    /// Replayable tool invocation.
    Call {
        /// Real wire digest or canonical relationship ordinal.
        identity: CallIdentity,
        /// Exact function name identity.
        name: Digest,
        /// Canonical foreign argument object identity.
        arguments: Digest,
        /// Exposed replayable thought signature identity.
        signature: Option<Digest>,
    },
    /// Matched replayable result.
    Result {
        /// Identity of the corresponding canonical invocation.
        identity: CallIdentity,
        /// Typed accepted result identity.
        content: Content,
        /// Error flag preserved by this client codec.
        is_error: bool,
    },
    /// Reasoning actually retained by the client parser, with optional family-bound payload.
    Reasoning {
        /// Exact replayable reasoning text identity.
        text: Digest,
        /// Preserved optional signature identity.
        signature: Option<Digest>,
        /// Canonical foreign replay payload identity.
        opaque: Option<Digest>,
        /// Replay family when the block contains a family-bound payload.
        family: Option<ReplayFamily>,
    },
}

/// Completed role run or a replayable state item at its exact history position.
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Unit {
    /// A maximal same-role run with all adjacent text concatenated.
    Message {
        /// Conversation role of the whole run.
        role: Role,
        /// Actual ordered digest-only parts.
        parts: Vec<Part>,
    },
    /// An opaque state boundary; same-role messages cannot merge across it.
    State {
        /// Codec replay family.
        family: ReplayFamily,
        /// Native state or sealed gateway carrier.
        kind: StateKind,
        /// Exact canonical replay payload identity.
        payload: Digest,
    },
}

/// An open role run: only digest metadata and one incremental trailing-text state.
#[derive(Clone)]
struct Run {
    /// Role being accumulated.
    role: Role,
    /// Completed non-text parts and finalized preceding text runs.
    parts: Vec<Part>,
    /// Adjacent trailing text, kept open through snapshots and future stream deltas.
    text: Option<Hmac<Sha256>>,
}

impl Run {
    /// Finalizes trailing text before the next distinct part; never inserts empty text.
    fn close_text(&mut self) {
        if let Some(text) = self.text.take() {
            self.parts.push(Part::Text {
                digest: finish(text),
            });
        }
    }

    /// Completes a cloned or owned run without retaining content bytes.
    fn unit(mut self) -> Unit {
        self.close_text();
        Unit::Message {
            role: self.role,
            parts: self.parts,
        }
    }
}

/// One strength's single-pass prefix chain, retaining the pre-tail chain for re-finalization.
#[derive(Clone)]
struct Path {
    /// Exact or ordinal tool identities.
    projection: Projection,
    /// Chain before the currently open role run.
    digest: Digest,
    /// Number of completed units before that open run.
    units: u64,
    /// Completed usable prefixes, including earlier branches.
    prefixes: Vec<HistoryPrefix>,
    /// Current nonempty role run.
    run: Option<Run>,
    /// False after an unresolved identity/relationship suffix.
    is_complete: bool,
    /// Whether completed units contain any assistant output or state.
    has_anchor: bool,
}

impl Path {
    /// Starts one empty-history chain.
    fn new(projection: Projection, key: &Key) -> Self {
        Self {
            projection,
            digest: key.root(),
            units: 0,
            prefixes: Vec::new(),
            run: None,
            is_complete: true,
            has_anchor: false,
        }
    }

    /// Adds a fully completed canonical unit exactly once to the chain.
    ///
    /// # Panics
    ///
    /// Panics on impossible request-local unit-count exhaustion.
    fn append(&mut self, key: &Key, unit: Unit) {
        self.digest = key.unit(self.digest, &unit);
        #[expect(
            clippy::expect_used,
            reason = "a resident request cannot contain u64::MAX units"
        )]
        let units = self
            .units
            .checked_add(1)
            .and_then(NonZeroU64::new)
            .expect("canonical unit count remains positive and cannot wrap");
        self.units = units.get();
        self.has_anchor |= matches!(
            unit,
            Unit::State { .. }
                | Unit::Message {
                    role: Role::Assistant,
                    ..
                }
        );
        self.prefixes.push(HistoryPrefix {
            projection: self.projection,
            digest: self.digest,
            units,
        });
    }

    /// Flushes a role only on a real part of another role or a state boundary.
    fn flush(&mut self, key: &Key) {
        if let Some(run) = self.run.take() {
            self.append(key, run.unit());
        }
    }

    /// Preserves the preceding valid unit before an unfingerprintable different-role suffix.
    fn stop(&mut self, key: &Key, role: Role) {
        if self.run.as_ref().is_some_and(|run| run.role != role) {
            self.flush(key);
        }
        self.run = None;
        self.is_complete = false;
    }

    /// Opens or extends a nonempty role run without changing inference message grouping.
    fn run(&mut self, key: &Key, role: Role) -> &mut Run {
        if self.run.as_ref().is_some_and(|run| run.role != role) {
            self.flush(key);
        }
        self.run.get_or_insert_with(|| Run {
            role,
            parts: Vec::new(),
            text: None,
        })
    }

    /// Non-destructive completion of the one open tail, never reserializing earlier prefixes.
    fn snapshot(&self, key: &Key) -> Self {
        let mut snapshot = self.clone();
        snapshot.flush(key);
        snapshot
    }
}

/// Request-local tool relationship bookkeeping, never a cached conversation object.
#[derive(Clone, Copy)]
struct Relationship {
    /// Canonical zero-based invocation index.
    ordinal: u64,
    /// Whether this call already consumed its codec-resolved result.
    is_matched: bool,
    /// Whether sequential legacy calls may reuse this codec-synthesized lookup identity.
    is_synthetic: bool,
}

/// Incremental text field retained until its codec's outward position is finalized.
/// A codec may clone a preceding adjacent field to preserve canonical text concatenation.
#[derive(Clone)]
pub(crate) struct PreparedText {
    /// Process-keyed UTF-8 field state, containing no original text bytes.
    authentication: Hmac<Sha256>,
    /// Whether this field includes an actual nonempty outward text delta.
    has_text: bool,
}

impl PreparedText {
    /// Finalizes a copy of the actual incremental field state without retaining its text.
    pub(crate) fn digest(&self) -> Digest {
        finish(self.authentication.clone())
    }

    /// Hashes an actual emitted text chunk without retaining it.
    pub(crate) fn update(&mut self, bytes: &[u8]) {
        self.has_text |= !bytes.is_empty();
        self.authentication.update(bytes);
    }
}

/// One reasoning part's finalized field digests, held until its outward position is known.
pub(crate) struct PreparedReasoning {
    /// Fixed-shape reasoning metadata, with every variable field replaced by a digest.
    part: Part,
}

/// One invocation's finalized field digests, held until its codec's outward order is known.
/// It retains no names, IDs, argument strings, or signature content.
pub(crate) struct PreparedCall {
    /// Codec-resolved internal relationship reference, already process-keyed.
    reference: Digest,
    /// Whether exact identity is unavailable because the source supplied no actual wire ID.
    is_synthetic: bool,
    /// Exact function name's field identity.
    name: Digest,
    /// Canonical foreign argument object's field identity.
    arguments: Digest,
    /// Optional replayed thought signature's field identity.
    signature: Option<Digest>,
}

/// Client-codec history projector, shared by inbound replay and actual outward result rendering.
/// Its working state contains no complete text, prompts, or generated response object.
#[derive(Clone)]
pub(crate) struct Projector {
    /// Stable process-local domain-separated hashing state.
    key: Key,
    /// Exact path, potentially unavailable after an id-less source identity.
    exact: Path,
    /// Relational path, potentially unavailable after an unresolved relationship.
    relational: Path,
    /// Hashed internal references resolve only the codec's already-established pairing.
    relationships: HashMap<Digest, Relationship>,
    /// Next canonical call ordinal.
    next_call: u64,
}

impl Projector {
    /// Creates a projector under the one startup key, stable across configuration reloads.
    pub(crate) fn new() -> Self {
        Self::with_key(PROCESS_KEY.clone())
    }

    /// Constructs request-local state, with a deterministic key available only inside this module.
    fn with_key(key: Key) -> Self {
        Self {
            exact: Path::new(Projection::Exact, &key),
            relational: Path::new(Projection::Relational, &key),
            key,
            relationships: HashMap::new(),
            next_call: 0,
        }
    }

    /// Hashes actual nonempty text chunks, including UTF-8 split across streamed byte boundaries.
    pub(crate) fn text(&mut self, role: Role, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        for path in [&mut self.exact, &mut self.relational] {
            if path.is_complete {
                path.run(&self.key, role)
                    .text
                    .get_or_insert_with(|| self.key.field(FieldDomain::Text))
                    .update(bytes);
            }
        }
    }

    /// Prepares an outward text field, optionally extending the existing same-role input tail.
    pub(crate) fn prepare_text(&self, role: Role, should_extend_tail: bool) -> PreparedText {
        let tail = should_extend_tail
            .then(|| {
                [&self.relational, &self.exact]
                    .into_iter()
                    .find_map(|path| {
                        path.run
                            .as_ref()
                            .filter(|run| path.is_complete && run.role == role)
                            .and_then(|run| run.text.clone())
                    })
            })
            .flatten();
        PreparedText {
            authentication: tail.unwrap_or_else(|| self.key.field(FieldDomain::Text)),
            has_text: false,
        }
    }

    /// Installs an ordered text field already seeded with its adjacent predecessor's state.
    pub(crate) fn text_prepared(&mut self, role: Role, text: PreparedText) {
        if !text.has_text {
            return;
        }
        for path in [&mut self.exact, &mut self.relational] {
            if path.is_complete {
                path.run(&self.key, role).text = Some(text.authentication.clone());
            }
        }
    }

    /// Appends one replayable digest-only part to a live projection.
    fn part(path: &mut Path, key: &Key, role: Role, part: Part) {
        if path.is_complete {
            let run = path.run(key, role);
            run.close_text();
            run.parts.push(part);
        }
    }

    /// Appends an ordinary part whose identity does not differ between strengths.
    fn common(&mut self, role: Role, part: Part) {
        Self::part(&mut self.exact, &self.key, role, part.clone());
        Self::part(&mut self.relational, &self.key, role, part);
    }

    /// Projects accepted inline data verbatim, without base64/MIME validation or media fetching.
    pub(crate) fn inline_image(&mut self, role: Role, mime_type: &str, data: &str) {
        self.common(
            role,
            Part::InlineImage {
                mime_type: self.key.bytes(FieldDomain::MimeType, mime_type.as_bytes()),
                data: self.key.bytes(FieldDomain::ImageData, data.as_bytes()),
            },
        );
    }

    /// Projects an accepted URL verbatim, without resolution or rewriting.
    pub(crate) fn url_image(&mut self, role: Role, url: &str) {
        self.common(
            role,
            Part::UrlImage {
                url: self.key.bytes(FieldDomain::Url, url.as_bytes()),
            },
        );
    }

    /// Projects a reasoning block only when the client codec actually preserves its replay.
    pub(crate) fn reasoning(
        &mut self,
        role: Role,
        family: ReplayFamily,
        text: &str,
        signature: Option<&str>,
        opaque: Option<&Value>,
    ) {
        let opaque = opaque.map(|value| {
            self.key.canonical_json(
                FieldDomain::OpaqueState,
                &ReasoningJson {
                    key: &self.key,
                    value,
                    mode: ReasoningJsonMode::root(family, value),
                },
            )
        });
        let text = self.key.bytes(FieldDomain::Text, text.as_bytes());
        self.reasoning_digest(role, family, text, signature, opaque);
    }

    /// Projects a codec-normalized borrowed reasoning payload without assembling its JSON text.
    ///
    /// # Panics
    ///
    /// Panics if accepted canonical fields cannot serialize to the infallible hashing sink.
    pub(crate) fn reasoning_canonical(
        &mut self,
        role: Role,
        family: ReplayFamily,
        text: &str,
        signature: Option<&str>,
        opaque: &impl Serialize,
    ) {
        self.reasoning_canonical_parts(role, family, std::iter::once(text), signature, opaque);
    }

    /// Hashes ordered summary fragments as one exact reasoning field without joining their text.
    ///
    /// # Panics
    ///
    /// Panics if accepted canonical fields cannot serialize to the infallible hashing sink.
    fn reasoning_canonical_parts<'a>(
        &mut self,
        role: Role,
        family: ReplayFamily,
        text: impl IntoIterator<Item = &'a str>,
        signature: Option<&str>,
        opaque: &impl Serialize,
    ) {
        let mut authentication = self.key.field(FieldDomain::Text);
        for fragment in text {
            authentication.update(fragment.as_bytes());
        }
        let opaque = self.key.canonical_json(FieldDomain::OpaqueState, opaque);
        self.reasoning_digest(
            role,
            family,
            finish(authentication),
            signature,
            Some(opaque),
        );
    }

    /// Prepares normalized complete reasoning without changing the current output position.
    ///
    /// # Panics
    ///
    /// Panics if accepted canonical fields cannot serialize to the infallible hashing sink.
    pub(crate) fn prepare_reasoning_canonical(
        &self,
        family: ReplayFamily,
        text: &str,
        signature: Option<&str>,
        opaque: &impl Serialize,
    ) -> PreparedReasoning {
        self.prepare_reasoning_digest(
            family,
            self.key.bytes(FieldDomain::Text, text.as_bytes()),
            signature,
            Some(self.key.canonical_json(FieldDomain::OpaqueState, opaque)),
        )
    }

    /// Hashes one known reasoning text field independently of late signature metadata.
    pub(crate) fn reasoning_text_digest(&self, text: &str) -> Digest {
        self.key.bytes(FieldDomain::Text, text.as_bytes())
    }

    /// Completes a known reasoning payload using only emitted field states and typed metadata.
    ///
    /// # Panics
    ///
    /// Panics if the codec's digest-only payload cannot serialize to the infallible hash sink.
    pub(crate) fn prepare_reasoning_fields(
        &self,
        family: ReplayFamily,
        text: &PreparedText,
        signature: Option<&str>,
        opaque: &impl Serialize,
    ) -> PreparedReasoning {
        self.prepare_reasoning_digest(
            family,
            text.digest(),
            signature,
            Some(self.key.canonical_json(FieldDomain::OpaqueState, opaque)),
        )
    }

    /// Appends completed reasoning at the codec's actual first-seen outward position.
    pub(crate) fn reasoning_prepared(&mut self, role: Role, reasoning: PreparedReasoning) {
        self.common(role, reasoning.part);
    }

    /// Appends already identified opaque reasoning beside the separately hashed visible fields.
    fn reasoning_digest(
        &mut self,
        role: Role,
        family: ReplayFamily,
        text: Digest,
        signature: Option<&str>,
        opaque: Option<Digest>,
    ) {
        let reasoning = self.prepare_reasoning_digest(family, text, signature, opaque);
        self.reasoning_prepared(role, reasoning);
    }

    /// Replaces variable reasoning fields with their identities without appending a role unit.
    fn prepare_reasoning_digest(
        &self,
        family: ReplayFamily,
        text: Digest,
        signature: Option<&str>,
        opaque: Option<Digest>,
    ) -> PreparedReasoning {
        PreparedReasoning {
            part: Part::Reasoning {
                text,
                signature: signature.map(|signature| {
                    self.key
                        .bytes(FieldDomain::Signatures, signature.as_bytes())
                }),
                opaque,
                family: (signature.is_some() || opaque.is_some()).then_some(family),
            },
        }
    }

    /// Preserves a replayable state boundary's kind, family, payload, and history position.
    pub(crate) fn state(&mut self, family: ReplayFamily, kind: StateKind, payload: &Value) {
        let payload = self.key.json(FieldDomain::OpaqueState, payload);
        self.state_digest(family, kind, payload);
    }

    /// Hashes a borrowed codec state view with lexically ordered members and canonical foreign
    /// fragments, without constructing a second state payload or copying its text.
    ///
    /// # Panics
    ///
    /// Panics if the accepted canonical view cannot serialize to the infallible hashing writer.
    pub(crate) fn state_canonical(
        &mut self,
        family: ReplayFamily,
        kind: StateKind,
        payload: &impl Serialize,
    ) {
        let prepared = self.prepare_state_canonical(family, kind, payload);
        self.state_prepared(prepared);
    }

    /// Digests a borrowed canonical codec state independently of its eventual output position.
    ///
    /// # Panics
    ///
    /// Panics if the accepted view cannot serialize to the infallible hashing writer.
    pub(crate) fn prepare_state_canonical(
        &self,
        family: ReplayFamily,
        kind: StateKind,
        payload: &impl Serialize,
    ) -> PreparedState {
        PreparedState {
            family,
            kind,
            payload: self.key.canonical_json(FieldDomain::OpaqueState, payload),
        }
    }

    /// Appends completed state digest fields at their actual codec output position.
    pub(crate) fn state_prepared(&mut self, state: PreparedState) {
        self.state_digest(state.family, state.kind, state.payload);
    }

    /// Appends only the authenticated carrier token, excluding outer resource metadata.
    ///
    /// # Panics
    ///
    /// Panics if the accepted string cannot serialize to the infallible hash sink.
    pub(crate) fn gateway_state(&mut self, family: ReplayFamily, carrier: &str) {
        let payload = self.key.json_string(FieldDomain::OpaqueState, carrier);
        self.state_digest(family, StateKind::Gateway, payload);
    }

    /// Process-keyed identity of an authenticated envelope's existing artifact ID.
    pub(crate) fn gateway_artifact(&self, artifact_id: &str) -> Digest {
        self.key
            .bytes(FieldDomain::OpaqueState, artifact_id.as_bytes())
    }

    /// Identity of an exact canonical producer-owned raw artifact, without origin inference.
    ///
    /// # Panics
    ///
    /// Panics if accepted foreign JSON cannot serialize to the infallible hash sink.
    pub(crate) fn native_artifact(&self, payload: &Value) -> Digest {
        self.key.json(FieldDomain::OpaqueState, payload)
    }

    /// Identifies a codec-normalized canonical artifact without constructing another JSON value.
    ///
    /// # Panics
    ///
    /// Panics if the typed borrowed view cannot serialize to the infallible hashing writer.
    pub(crate) fn canonical_artifact(&self, payload: &impl Serialize) -> Digest {
        self.key.canonical_json(FieldDomain::OpaqueState, payload)
    }

    /// Frames an already hashed state field onto both history projections.
    fn state_digest(&mut self, family: ReplayFamily, kind: StateKind, payload: Digest) {
        for path in [&mut self.exact, &mut self.relational] {
            if path.is_complete {
                path.flush(&self.key);
                path.append(
                    &self.key,
                    Unit::State {
                        family,
                        kind,
                        payload,
                    },
                );
            }
        }
    }

    /// Projects one actual invocation. Duplicate unresolved identities make only the suffix
    /// unavailable; sequential matched legacy references preserve the codec's existing pairing.
    ///
    /// # Panics
    ///
    /// Panics on impossible resident call-count exhaustion.
    pub(crate) fn call(
        &mut self,
        role: Role,
        identity: Identity<'_>,
        name: &str,
        arguments: &Value,
        signature: Option<&str>,
    ) {
        let prepared = self.prepare_call(identity, name, arguments, signature);
        self.call_prepared(role, prepared);
    }

    /// Finalizes invocation fields without assigning its conversation ordinal yet.
    ///
    /// # Panics
    ///
    /// Panics if accepted foreign JSON cannot serialize to the infallible hash writer.
    pub(crate) fn prepare_call(
        &self,
        identity: Identity<'_>,
        name: &str,
        arguments: &Value,
        signature: Option<&str>,
    ) -> PreparedCall {
        PreparedCall {
            reference: self
                .key
                .bytes(FieldDomain::CallId, identity.resolved().as_bytes()),
            is_synthetic: matches!(identity, Identity::Synthetic(_)),
            name: self.key.bytes(FieldDomain::Name, name.as_bytes()),
            arguments: self.key.json(FieldDomain::Arguments, arguments),
            signature: signature.map(|signature| {
                self.key
                    .bytes(FieldDomain::Signatures, signature.as_bytes())
            }),
        }
    }

    /// Appends finalized invocation fields in actual codec history order, assigning ordinals now.
    ///
    /// # Panics
    ///
    /// Panics on impossible resident call-count exhaustion.
    pub(crate) fn call_prepared(&mut self, role: Role, prepared: PreparedCall) {
        let PreparedCall {
            reference,
            is_synthetic,
            name,
            arguments,
            signature,
        } = prepared;
        if self
            .relationships
            .get(&reference)
            .is_some_and(|previous| !(is_synthetic && previous.is_synthetic && previous.is_matched))
        {
            self.exact.stop(&self.key, role);
            self.relational.stop(&self.key, role);
            return;
        }
        let ordinal = self.next_call;
        #[expect(
            clippy::expect_used,
            reason = "a resident history cannot contain u64::MAX calls"
        )]
        {
            self.next_call = self
                .next_call
                .checked_add(1)
                .expect("call ordinals cannot wrap");
        }
        self.relationships.insert(
            reference,
            Relationship {
                ordinal,
                is_matched: false,
                is_synthetic,
            },
        );
        if !is_synthetic {
            Self::part(
                &mut self.exact,
                &self.key,
                role,
                Part::Call {
                    identity: CallIdentity::Exact(reference),
                    name,
                    arguments,
                    signature,
                },
            );
        } else {
            self.exact.stop(&self.key, role);
        }
        Self::part(
            &mut self.relational,
            &self.key,
            role,
            Part::Call {
                identity: CallIdentity::Relational(ordinal),
                name,
                arguments,
                signature,
            },
        );
    }

    /// Projects a result against the codec-resolved call, without inventing another pairing rule.
    pub(crate) fn result(
        &mut self,
        role: Role,
        identity: Identity<'_>,
        content: ResultContent<'_>,
        is_error: bool,
    ) {
        let content = match content {
            ResultContent::Text(text) => {
                Content::Text(self.key.bytes(FieldDomain::Results, text.as_bytes()))
            }
            ResultContent::Json(value) => Content::Json(self.key.json(FieldDomain::Results, value)),
        };
        self.result_digest(role, identity, content, is_error);
    }

    /// Projects codec-normalized text chunks without assembling the joined result solely for
    /// affinity. Separators are supplied by the codec's accepted text-part semantics.
    pub(crate) fn result_text_chunks<'a>(
        &mut self,
        role: Role,
        identity: Identity<'_>,
        chunks: impl IntoIterator<Item = &'a str>,
        is_error: bool,
    ) {
        let mut authentication = self.key.field(FieldDomain::Results);
        for chunk in chunks {
            authentication.update(chunk.as_bytes());
        }
        self.result_digest(
            role,
            identity,
            Content::Text(finish(authentication)),
            is_error,
        );
    }

    /// Applies the existing request-local relationship rule to already digested result content.
    fn result_digest(
        &mut self,
        role: Role,
        identity: Identity<'_>,
        content: Content,
        is_error: bool,
    ) {
        let reference = self
            .key
            .bytes(FieldDomain::CallId, identity.resolved().as_bytes());
        let Some(relationship) = self
            .relationships
            .get_mut(&reference)
            .filter(|call| !call.is_matched)
        else {
            self.exact.stop(&self.key, role);
            self.relational.stop(&self.key, role);
            return;
        };
        relationship.is_matched = true;
        let ordinal = relationship.ordinal;
        let can_project_exact = !relationship.is_synthetic && matches!(identity, Identity::Wire(_));
        if can_project_exact {
            Self::part(
                &mut self.exact,
                &self.key,
                role,
                Part::Result {
                    identity: CallIdentity::Exact(reference),
                    content: content.clone(),
                    is_error,
                },
            );
        } else {
            self.exact.stop(&self.key, role);
        }
        Self::part(
            &mut self.relational,
            &self.key,
            role,
            Part::Result {
                identity: CallIdentity::Relational(ordinal),
                content,
                is_error,
            },
        );
    }

    /// Reads every usable prefix and the strongest complete retry, without closing the input tail.
    pub(crate) fn evidence(&self) -> Evidence {
        let exact = self.exact.snapshot(&self.key);
        let relational = self.relational.snapshot(&self.key);
        let mut history = exact.prefixes.clone();
        for (prefix, exact) in relational.prefixes.iter().zip(
            exact
                .prefixes
                .iter()
                .map(Some)
                .chain(std::iter::repeat(None)),
        ) {
            if exact
                .is_none_or(|exact| exact.units != prefix.units || exact.digest != prefix.digest)
            {
                history.push(*prefix);
            }
        }
        let retry = [exact, relational]
            .into_iter()
            .find(|path| path.is_complete)
            .map(|path| RetryFingerprint {
                projection: path.projection,
                digest: path.digest,
            });
        Evidence {
            history,
            retry,
            ..Evidence::default()
        }
    }

    /// Returns final fully replayable output/history anchors; a user-only run is never established.
    /// Callers invoke this only after the codec's actual output completes, before terminal
    /// delivery.
    pub(crate) fn completed_anchors(&self) -> Vec<Anchor> {
        let exact = self.exact.snapshot(&self.key);
        let relational = self.relational.snapshot(&self.key);
        let mut anchors = Vec::new();
        let mut previous = None;
        for path in [exact, relational] {
            if path.is_complete
                && path.has_anchor
                && previous != Some(path.digest)
                && let Some(prefix) = path.prefixes.last()
            {
                anchors.push(Anchor::History(*prefix));
                previous = Some(path.digest);
            }
        }
        anchors
    }
}
