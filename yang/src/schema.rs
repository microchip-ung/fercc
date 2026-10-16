// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

//! Semantic interpretation of parsed YANG statement trees into a schema
//! tree with attached SIDs: grouping/uses instantiation, augment, typedef
//! resolution, identity graph, leafref resolution.
//!
//! Ported from `support/yang-enc/yang-utils.rb`'s `Yang::ModuleSet` /
//! `Module` / `Statement` / `Type` / `Identity` classes -- same feature
//! set, not more (no `when`/`must`, only the `yang-data` and `structure`
//! extensions, `deviate add/delete` unimplemented), cross-checked
//! structurally against `sw-velocitydrive-devclient`'s
//! `src/yang/yang-utils.ts`.
//!
//! Arena-based (`Vec<Node>` etc. plus `NodeId` indices) rather than an
//! object graph with parent pointers, which is the natural shape in Rust.

use std::collections::HashMap;

use crate::parser::{self, Stmt};
use crate::sid::{self, Namespace};

type NodeId = usize;
type IdentityId = usize;
type TypedefId = usize;

type Scope<'a> = [(&'a Stmt, String)];
type ScopeVec<'a> = Vec<(&'a Stmt, String)>;

struct PendingTypedefType<'a> {
    resolve: String,
    scope: ScopeVec<'a>,
    stmt: &'a Stmt,
}

const SCHEMA_NODE_KEYWORDS: &[&str] = &[
    "container",
    "leaf",
    "leaf-list",
    "list",
    "choice",
    "case",
    "rpc",
    "action",
    "input",
    "output",
    "notification",
    "anydata",
    "anyxml",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Current,
    Deprecated,
    Obsolete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    Binary,
    Bits,
    Boolean,
    Decimal64,
    Empty,
    Enumeration,
    Identityref,
    InstanceIdentifier,
    Int8,
    Int16,
    Int32,
    Int64,
    Uint8,
    Uint16,
    Uint32,
    Uint64,
    Leafref,
    String,
    Union,
}

impl Builtin {
    fn from_name(name: &str) -> Option<Builtin> {
        Some(match name {
            "binary" => Builtin::Binary,
            "bits" => Builtin::Bits,
            "boolean" => Builtin::Boolean,
            "decimal64" => Builtin::Decimal64,
            "empty" => Builtin::Empty,
            "enumeration" => Builtin::Enumeration,
            "identityref" => Builtin::Identityref,
            "instance-identifier" => Builtin::InstanceIdentifier,
            "int8" => Builtin::Int8,
            "int16" => Builtin::Int16,
            "int32" => Builtin::Int32,
            "int64" => Builtin::Int64,
            "uint8" => Builtin::Uint8,
            "uint16" => Builtin::Uint16,
            "uint32" => Builtin::Uint32,
            "uint64" => Builtin::Uint64,
            "leafref" => Builtin::Leafref,
            "string" => Builtin::String,
            "union" => Builtin::Union,
            _ => return None,
        })
    }

    pub fn is_64bit_int(self) -> bool {
        matches!(self, Builtin::Int64 | Builtin::Uint64)
    }
}

#[derive(Debug, Clone)]
pub struct Bit {
    pub name: String,
    pub position: u32,
}

#[derive(Debug, Clone)]
pub struct EnumVal {
    pub name: String,
    pub value: i64,
}

#[derive(Debug, Clone)]
pub struct TypeDescriptor {
    pub builtin: Builtin,
    pub fraction_digits: Option<u8>,
    pub bits: Vec<Bit>,
    pub enums: Vec<EnumVal>,
    identity_bases: Vec<IdentityId>,
    /// The module that declared this identityref usage -- an unprefixed
    /// identity value name is disambiguated against this module.
    pub source_module: Option<String>,
    pub leafref_path: Option<String>,
    leafref_target: Option<NodeId>,
    union_members: Vec<TypeDescriptor>,
    /// `range` restrictions (int8/16/32/64, uint8/16/32/64) or `length`
    /// restrictions (string, binary) -- one underlying field for both,
    /// mirroring `Type#ranges`/`Type#length` in yang-utils.rb (`length` is
    /// literally `alias :length :ranges` there). Starts at the builtin's
    /// own natural bounds (`default_ranges`) and is narrowed by each
    /// `range`/`length` statement encountered while resolving a type,
    /// innermost (builtin or typedef base) first. Empty for every other
    /// builtin -- nothing in this port ever restricts or reads a range
    /// for them.
    pub ranges: Vec<(i128, i128)>,
    /// `pattern` restrictions (string only), accumulated top-down through
    /// the typedef chain (RFC 7950: multiple pattern statements, whether
    /// inherited or local, are all ANDed together).
    pub patterns: Vec<String>,
    base: Option<TypedefId>,
    identity_values: Option<std::collections::HashSet<IdentityId>>,
}

impl TypeDescriptor {
    pub(crate) fn new(builtin: Builtin) -> Self {
        Self {
            builtin,
            fraction_digits: None,
            bits: Vec::new(),
            enums: Vec::new(),
            identity_bases: Vec::new(),
            source_module: None,
            leafref_path: None,
            leafref_target: None,
            union_members: Vec::new(),
            ranges: Vec::new(),
            patterns: Vec::new(),
            base: None,
            identity_values: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Typedef {
    pub name: String,
    pub module: String,
    pub scope: String,
    pub description: Option<String>,
    ty: Option<TypeDescriptor>,
}

/// The natural bounds a builtin type starts with before any `range`/
/// `length` statement narrows them, mirroring `Type#initialize`
/// (yang-utils.rb:654-661). Every other builtin starts with no ranges at
/// all -- there is nothing to narrow and nothing downstream reads them.
pub(crate) fn default_ranges(builtin: Builtin) -> Vec<(i128, i128)> {
    match builtin {
        Builtin::Int8 => vec![(-128, 127)],
        Builtin::Int16 => vec![(-32768, 32767)],
        Builtin::Int32 => vec![(-2147483648, 2147483647)],
        Builtin::Int64 => vec![(-9223372036854775808, 9223372036854775807)],
        Builtin::Uint8 => vec![(0, 255)],
        Builtin::Uint16 => vec![(0, 65535)],
        Builtin::Uint32 => vec![(0, 4294967295)],
        Builtin::Uint64 | Builtin::Binary | Builtin::String => vec![(0, 18446744073709551615)],
        _ => Vec::new(),
    }
}

/// Parse a `range`/`length` argument (`range-part *("|" range-part)`,
/// each part `bound [".." bound]`, a bound being a decimal integer or the
/// keywords `min`/`max`) and narrow `prev` accordingly, mirroring
/// `Type#add_range`/`Type#interpret_range_bound` (yang-utils.rb:702-723).
/// `min`/`max` resolve against `prev`'s own extremes, not the builtin's
/// absolute bounds, so restrictions compose correctly across a typedef
/// chain. Malformed bounds fall back to `prev`'s minimum rather than
/// panicking -- this port has no user-facing "malformed YANG" diagnostic
/// path for schema data, and every catalog this ships against is already
/// known-good.
fn restrict_range(prev: &[(i128, i128)], spec: &str) -> Vec<(i128, i128)> {
    let prev_min = prev.iter().map(|r| r.0).min().unwrap_or(0);
    let prev_max = prev.iter().map(|r| r.1).max().unwrap_or(0);
    let resolve = |tok: &str| -> i128 {
        match tok {
            "min" => prev_min,
            "max" => prev_max,
            _ => tok.parse().unwrap_or(prev_min),
        }
    };
    spec.split('|')
        .map(|part| {
            let mut bounds = part.split("..").map(str::trim);
            let min_tok = bounds.next().unwrap_or("");
            let max_tok = bounds.next().unwrap_or(min_tok);
            (resolve(min_tok), resolve(max_tok))
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct Identity {
    pub name: String,
    pub module: String,
    pub description: Option<String>,
    base: Vec<IdentityId>,
    derived: Vec<IdentityId>,
    pub sid: Option<i64>,
}

fn transitive_derived_excluding_self(
    identities: &[Identity],
    start: IdentityId,
) -> std::collections::HashSet<IdentityId> {
    let mut out = std::collections::HashSet::new();
    let mut stack: Vec<IdentityId> = identities[start].derived.clone();
    while let Some(id) = stack.pop() {
        if out.insert(id) {
            stack.extend(identities[id].derived.iter().copied());
        }
    }
    out
}

fn derived_from_all(
    identities: &[Identity],
    bases: &[IdentityId],
) -> std::collections::HashSet<IdentityId> {
    let mut iter = bases.iter();
    let Some(&first) = iter.next() else {
        return std::collections::HashSet::new();
    };
    let mut out = transitive_derived_excluding_self(identities, first);
    for &base in iter {
        let other = transitive_derived_excluding_self(identities, base);
        out.retain(|id| other.contains(id));
    }
    out
}

fn collect_identity_values(ty: &TypeDescriptor, out: &mut std::collections::HashSet<IdentityId>) {
    if let Some(values) = &ty.identity_values {
        out.extend(values.iter().copied());
    }
    for member in &ty.union_members {
        collect_identity_values(member, out);
    }
}

#[derive(Debug, Clone)]
pub struct Node {
    pub kw: String,
    /// Equivalent to Ruby's `Statement#arg`: `"module:local"` for a
    /// top-level node or one placed by an augment crossing a module
    /// boundary, bare `local` name otherwise. IID/schema-node-id path
    /// segments must match this exactly.
    pub name: String,
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    pub config: bool,
    pub mandatory: bool,
    ty: Option<TypeDescriptor>,
    keys: Vec<NodeId>,
    pub sid: Option<i64>,
    pub owner_module: String,
    /// This node's own `description` substatement text, if any --
    /// tracked only for [`crate::json_schema`]'s benefit (matches
    /// `get_description`, yang-utils.rb:549-555: the first `description`
    /// substatement's text, verbatim, no further processing).
    pub description: Option<String>,
    pub status: Option<Status>,
    pub default: Option<String>,
    pub synthetic: bool,
    references_modules: Vec<String>,
}

impl Node {
    pub fn local_name(&self) -> &str {
        self.name
            .rsplit_once(':')
            .map(|(_, n)| n)
            .unwrap_or(&self.name)
    }
}

pub struct Module {
    pub name: String,
    pub namespace: String,
    pub revision: Option<String>,
    pub sid: Option<i64>,
    root_nodes: Vec<NodeId>,
    pub deviated_by: std::collections::HashSet<String>,
}

pub struct YangData {
    pub module: String,
    pub name: String,
    root: NodeId,
}

impl YangData {
    fn qualified_name(&self) -> String {
        format!("{}:{}", self.module, self.name)
    }
}

#[derive(Clone, Copy)]
pub struct YangDataRef<'a> {
    schema: &'a Schema,
    yang_data: &'a YangData,
}

impl<'a> std::ops::Deref for YangDataRef<'a> {
    type Target = YangData;
    fn deref(&self) -> &YangData {
        self.yang_data
    }
}

impl<'a> YangDataRef<'a> {
    pub fn root(&self) -> NodeRef<'a> {
        self.schema.node_ref(self.yang_data.root)
    }
}

pub struct ImportOnlyModule {
    pub name: String,
    pub namespace: String,
    pub import_revision_date: Option<String>,
    pub sid: Option<i64>,
}

#[derive(Clone, Copy)]
pub struct ModuleRef<'a> {
    schema: &'a Schema,
    module: &'a Module,
}

impl<'a> std::ops::Deref for ModuleRef<'a> {
    type Target = Module;
    fn deref(&self) -> &Module {
        self.module
    }
}

impl<'a> ModuleRef<'a> {
    pub fn root_nodes(&self) -> impl Iterator<Item = NodeRef<'a>> + 'a {
        let schema = self.schema;
        self.module
            .root_nodes
            .iter()
            .map(move |&id| schema.node_ref(id))
    }

    pub fn root_data_nodes(&self) -> impl Iterator<Item = NodeRef<'a>> + 'a {
        let schema = self.schema;
        spliced(&schema.nodes, &self.module.root_nodes)
            .into_iter()
            .map(move |id| schema.node_ref(id))
    }
}

pub struct Schema {
    nodes: Vec<Node>,
    typedefs: Vec<Typedef>,
    identities: Vec<Identity>,
    module_root: HashMap<String, Module>,
    import_only_modules: Vec<ImportOnlyModule>,
    yang_data: Vec<YangData>,
    structure_roots: HashMap<String, NodeId>,
    sid_index: HashMap<i64, NodeId>,
    reachable_typedefs: std::collections::HashSet<TypedefId>,
    reachable_identities: std::collections::HashSet<IdentityId>,
    implemented_identities: std::collections::HashSet<IdentityId>,
}

#[derive(Clone, Copy)]
pub struct NodeRef<'a> {
    schema: &'a Schema,
    id: NodeId,
}

impl<'a> std::ops::Deref for NodeRef<'a> {
    type Target = Node;
    fn deref(&self) -> &Node {
        &self.schema.nodes[self.id]
    }
}

impl<'a> NodeRef<'a> {
    pub fn children(&self) -> impl Iterator<Item = NodeRef<'a>> + 'a {
        let schema = self.schema;
        schema.nodes[self.id]
            .children
            .iter()
            .map(move |&c| NodeRef { schema, id: c })
    }

    pub fn data_children(&self) -> impl Iterator<Item = NodeRef<'a>> + 'a {
        let schema = self.schema;
        spliced(&schema.nodes, &schema.nodes[self.id].children)
            .into_iter()
            .map(move |id| NodeRef { schema, id })
    }

    /// List keys, in the order the `key` statement itself lists them --
    /// not necessarily the order the key leafs are declared in, and not
    /// necessarily input order either.
    pub fn keys(&self) -> impl Iterator<Item = NodeRef<'a>> + 'a {
        let schema = self.schema;
        schema.nodes[self.id]
            .keys
            .iter()
            .map(move |&id| NodeRef { schema, id })
    }

    pub fn parent(&self) -> Option<NodeRef<'a>> {
        let schema = self.schema;
        schema.nodes[self.id]
            .parent
            .map(|p| NodeRef { schema, id: p })
    }

    pub fn find_child(&self, name: &str) -> Option<NodeRef<'a>> {
        self.children().find(|c| c.name == name)
    }

    pub fn find_data_child(&self, name: &str) -> Option<NodeRef<'a>> {
        self.data_children().find(|c| c.name == name)
    }

    pub fn ty(&self) -> Option<TypeRef<'a>> {
        let schema = self.schema;
        schema.nodes[self.id]
            .ty
            .as_ref()
            .map(|ty| TypeRef { schema, ty })
    }

    pub fn path(&self) -> String {
        node_path_of(&self.schema.nodes, self.id)
    }
}

#[derive(Clone, Copy)]
pub struct TypeRef<'a> {
    schema: &'a Schema,
    ty: &'a TypeDescriptor,
}

impl<'a> std::ops::Deref for TypeRef<'a> {
    type Target = TypeDescriptor;
    fn deref(&self) -> &TypeDescriptor {
        self.ty
    }
}

impl<'a> TypeRef<'a> {
    pub fn union_members(&self) -> impl Iterator<Item = TypeRef<'a>> + 'a {
        let schema = self.schema;
        self.ty
            .union_members
            .iter()
            .map(move |ty| TypeRef { schema, ty })
    }

    pub fn identity_bases(&self) -> impl Iterator<Item = IdentityRef<'a>> + 'a {
        let schema = self.schema;
        self.ty
            .identity_bases
            .iter()
            .map(move |&id| IdentityRef { schema, id })
    }

    pub fn leafref_target(&self) -> Option<NodeRef<'a>> {
        let schema = self.schema;
        self.ty.leafref_target.map(|id| NodeRef { schema, id })
    }

    pub fn identity_values(&self) -> Option<std::collections::HashSet<IdentityRef<'a>>> {
        let schema = self.schema;
        self.ty.identity_values.as_ref().map(|values| {
            values
                .iter()
                .map(|&id| IdentityRef { schema, id })
                .collect()
        })
    }

    pub fn base(&self) -> Option<TypedefRef<'a>> {
        let schema = self.schema;
        self.ty.base.map(|id| TypedefRef { schema, id })
    }
}

#[derive(Clone, Copy)]
pub struct TypedefRef<'a> {
    schema: &'a Schema,
    id: TypedefId,
}

impl<'a> std::ops::Deref for TypedefRef<'a> {
    type Target = Typedef;
    fn deref(&self) -> &Typedef {
        &self.schema.typedefs[self.id]
    }
}

impl<'a> TypedefRef<'a> {
    pub fn ty(&self) -> TypeRef<'a> {
        TypeRef {
            schema: self.schema,
            ty: self.schema.typedefs[self.id]
                .ty
                .as_ref()
                .expect("a typedef's own type is always interpreted by the time a Schema exists"),
        }
    }
}

#[derive(Clone, Copy)]
pub struct IdentityRef<'a> {
    schema: &'a Schema,
    id: IdentityId,
}

impl<'a> std::ops::Deref for IdentityRef<'a> {
    type Target = Identity;
    fn deref(&self) -> &Identity {
        &self.schema.identities[self.id]
    }
}

impl<'a> PartialEq for IdentityRef<'a> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.schema, other.schema) && self.id == other.id
    }
}
impl<'a> Eq for IdentityRef<'a> {}
impl<'a> std::hash::Hash for IdentityRef<'a> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (self.schema as *const Schema).hash(state);
        self.id.hash(state);
    }
}

impl<'a> IdentityRef<'a> {
    pub fn base(&self) -> impl Iterator<Item = IdentityRef<'a>> + 'a {
        let schema = self.schema;
        schema.identities[self.id]
            .base
            .iter()
            .map(move |&id| IdentityRef { schema, id })
    }

    pub fn is_implemented(&self) -> bool {
        self.schema.implemented_identities.contains(&self.id)
    }
}

fn node_path_of(nodes: &[Node], id: NodeId) -> String {
    let mut chain = Vec::new();
    let mut cur = Some(id);
    while let Some(n) = cur {
        chain.push(n);
        cur = nodes[n].parent;
    }
    chain.reverse();
    chain
        .into_iter()
        .map(|n| nodes[n].name.as_str())
        .fold(String::new(), |mut path, name| {
            path.push('/');
            path.push_str(name);
            path
        })
}

fn spliced(nodes: &[Node], ids: &[NodeId]) -> Vec<NodeId> {
    let mut out = Vec::new();
    for &id in ids {
        if matches!(nodes[id].kw.as_str(), "choice" | "case") {
            out.extend(spliced(nodes, &nodes[id].children));
        } else {
            out.push(id);
        }
    }
    out
}

fn node_namespace_module(nodes: &[Node], id: NodeId) -> &str {
    nodes[id]
        .name
        .split_once(':')
        .map(|(m, _)| m)
        .unwrap_or(&nodes[id].owner_module)
}

fn segment_matches(nodes: &[Node], seg: &str, candidate: NodeId) -> bool {
    match seg.split_once(':') {
        Some((module, local)) => {
            nodes[candidate].local_name() == local
                && node_namespace_module(nodes, candidate) == module
        }
        None => nodes[candidate].local_name() == seg,
    }
}

fn find_child_by_local_raw(nodes: &[Node], parent: NodeId, seg: &str) -> Option<NodeId> {
    nodes[parent]
        .children
        .iter()
        .copied()
        .find(|&c| segment_matches(nodes, seg, c))
}

fn find_data_child_by_local_raw(nodes: &[Node], parent: NodeId, seg: &str) -> Option<NodeId> {
    spliced(nodes, &nodes[parent].children)
        .into_iter()
        .find(|&c| segment_matches(nodes, seg, c))
}

fn descend_by_local<'s>(
    nodes: &[Node],
    start: NodeId,
    segs: impl Iterator<Item = &'s str>,
) -> Option<NodeId> {
    let mut cur = start;
    for seg in segs {
        cur = find_child_by_local_raw(nodes, cur, seg)?;
    }
    Some(cur)
}

fn descend_by_data_local<'s>(
    nodes: &[Node],
    start: NodeId,
    segs: impl Iterator<Item = &'s str>,
) -> Option<NodeId> {
    let mut cur = start;
    for seg in segs {
        cur = find_data_child_by_local_raw(nodes, cur, seg)?;
    }
    Some(cur)
}

fn resolve_schema_path(
    nodes: &[Node],
    module_root: &HashMap<String, Module>,
    path: &str,
) -> Option<NodeId> {
    let mut segs = path.split('/').filter(|s| !s.is_empty());
    let first = segs.next()?;
    let (module_name, first_local) = first.split_once(':')?;
    let module = module_root.get(module_name)?;
    let first_node = module
        .root_nodes
        .iter()
        .copied()
        .find(|&id| nodes[id].local_name() == first_local)?;
    descend_by_local(nodes, first_node, segs)
}

fn resolve_yang_data_path(
    nodes: &[Node],
    roots: impl IntoIterator<Item = NodeId>,
    path: &str,
) -> Option<NodeId> {
    let mut segs = path.split('/').filter(|s| !s.is_empty());
    let first = segs.next()?;
    let first_node = roots.into_iter().find(|&id| nodes[id].name == first)?;
    descend_by_local(nodes, first_node, segs)
}

impl Schema {
    fn node_ref(&self, id: NodeId) -> NodeRef<'_> {
        NodeRef { schema: self, id }
    }

    pub fn node_by_sid(&self, sid: i64) -> Option<NodeRef<'_>> {
        self.sid_index.get(&sid).map(|&id| self.node_ref(id))
    }

    pub fn identity_by_sid(&self, sid: i64) -> Option<IdentityRef<'_>> {
        self.identities
            .iter()
            .position(|i| i.sid == Some(sid))
            .map(|id| IdentityRef { schema: self, id })
    }

    pub fn identity_by_name(&self, module: &str, name: &str) -> Option<IdentityRef<'_>> {
        self.identities()
            .find(|i| i.module == module && i.name == name)
    }

    pub fn typedef_by_name(&self, scope: &str, name: &str) -> Option<TypedefRef<'_>> {
        self.typedefs().find(|t| t.scope == scope && t.name == name)
    }

    pub fn typedefs(&self) -> impl Iterator<Item = TypedefRef<'_>> + '_ {
        (0..self.typedefs.len())
            .filter(|id| self.reachable_typedefs.contains(id))
            .map(|id| TypedefRef { schema: self, id })
    }

    pub fn identities(&self) -> impl Iterator<Item = IdentityRef<'_>> + '_ {
        (0..self.identities.len())
            .filter(|id| self.reachable_identities.contains(id))
            .map(|id| IdentityRef { schema: self, id })
    }

    pub fn modules(&self) -> impl Iterator<Item = ModuleRef<'_>> + '_ {
        self.module_root.values().map(|module| ModuleRef {
            schema: self,
            module,
        })
    }

    pub fn module_by_name(&self, name: &str) -> Option<ModuleRef<'_>> {
        self.module_root.get(name).map(|module| ModuleRef {
            schema: self,
            module,
        })
    }

    pub fn module_by_sid(&self, sid: i64) -> Option<ModuleRef<'_>> {
        self.module_root
            .values()
            .find(|m| m.sid == Some(sid))
            .map(|module| ModuleRef {
                schema: self,
                module,
            })
    }

    pub fn import_only_modules(&self) -> impl Iterator<Item = &ImportOnlyModule> + '_ {
        self.import_only_modules.iter()
    }

    pub fn node_by_schema_path(&self, path: &str) -> Option<NodeRef<'_>> {
        let id = resolve_schema_path(&self.nodes, &self.module_root, path)?;
        Some(self.node_ref(id))
    }

    pub fn yang_data(&self) -> impl Iterator<Item = YangDataRef<'_>> + '_ {
        self.yang_data.iter().map(|yang_data| YangDataRef {
            schema: self,
            yang_data,
        })
    }

    pub fn structure_roots(&self) -> impl Iterator<Item = NodeRef<'_>> + '_ {
        self.structure_roots.values().map(|&id| self.node_ref(id))
    }

    pub fn node_by_data_path(&self, path: &str) -> Option<NodeRef<'_>> {
        let mut segs = path.split('/').filter(|s| !s.is_empty());
        let first = segs.next()?;
        let first_node = self.root_data_node_by_name(first)?;
        let id = descend_by_data_local(&self.nodes, first_node.id, segs)?;
        Some(self.node_ref(id))
    }

    pub fn root_data_node_by_name(&self, name: &str) -> Option<NodeRef<'_>> {
        let id = self
            .module_root
            .values()
            .flat_map(|m| spliced(&self.nodes, &m.root_nodes))
            .find(|&id| self.nodes[id].name == name)?;
        Some(self.node_ref(id))
    }
}

#[derive(Debug)]
pub struct SchemaError(pub String);
impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for SchemaError {}
impl From<String> for SchemaError {
    fn from(s: String) -> Self {
        SchemaError(s)
    }
}

type R<T> = Result<T, SchemaError>;

fn err(msg: impl Into<String>) -> SchemaError {
    SchemaError(msg.into())
}

fn read_status(s: &Stmt) -> R<Status> {
    match s.sub("status") {
        Some(st) => match st.arg_str() {
            "current" => Ok(Status::Current),
            "deprecated" => Ok(Status::Deprecated),
            "obsolete" => Ok(Status::Obsolete),
            other => Err(err(format!("unknown status {other:?}"))),
        },
        None => Ok(Status::Current),
    }
}

struct ModuleCtx {
    prefix_to_module: HashMap<String, String>,
}

impl ModuleCtx {
    fn resolve_prefix(&self, prefix: &str) -> Option<&str> {
        self.prefix_to_module.get(prefix).map(|s| s.as_str())
    }
}

struct Builder<'a> {
    raw_modules: HashMap<String, &'a Stmt>,
    contexts: HashMap<String, ModuleCtx>,
    building: std::collections::HashSet<String>,
    built: std::collections::HashSet<String>,

    nodes: Vec<Node>,
    typedefs: Vec<Typedef>,
    identities: Vec<Identity>,
    identity_by_module_name: HashMap<(String, String), IdentityId>,
    /// Key: `(typedef_scope, name)`, where `typedef_scope` is the
    /// label of the scope frame the typedef was declared directly in
    /// (a module name, a schema node's own path, or a use site's path
    /// for one declared in a spliced-in grouping). Value: this
    /// typedef's identity, plus everything needed to interpret it.
    typedef_cache: HashMap<(String, String), (TypedefId, PendingTypedefType<'a>)>,
    module_root: HashMap<String, Module>,
    yang_data: Vec<YangData>,
    structure_roots: HashMap<String, NodeId>,
    implemented_yang_data: std::collections::HashSet<String>,
    identity_accept_lists: HashMap<String, Vec<String>>,
    identityref_accept_lists: Vec<(String, Vec<String>)>,
    reachable_typedefs: std::collections::HashSet<TypedefId>,
    reachable_identities: std::collections::HashSet<IdentityId>,
    implemented_identities: std::collections::HashSet<IdentityId>,
    referenced_modules: std::collections::HashSet<String>,
}

/// Parse `.yang` sources and build the full schema, including flattening
/// and SID attachment from `.sid` sources. Fresh every call -- no
/// caching, by design.
///
/// `implemented_yang_data` is the device-specific allow-list of
/// yang-data template names (e.g. "ietf-coreconf:coreconf-error")
/// that this device's wire protocol actually serves -- see `finish()`.
/// Whether a given template is "protocol accessible" can't be derived
/// from the YANG text itself, so the
/// caller must supply this real, separately-maintained fact.
pub fn build(
    yang_sources: &[String],
    sid_sources: &[String],
    implemented_yang_data: &[&str],
    identity_accept_lists: &[(&str, &[&str])],
    identityref_accept_lists: &[(&str, &[&str])],
) -> R<Schema> {
    let parsed: Vec<Stmt> = yang_sources
        .iter()
        .map(|s| parser::parse_module(s).map_err(|e| err(e.to_string())))
        .collect::<R<Vec<_>>>()?;

    let mut raw_modules = HashMap::new();
    for m in &parsed {
        raw_modules.insert(m.arg_str().to_string(), m);
    }

    let mut contexts = HashMap::new();
    for m in &parsed {
        let mname = m.arg_str().to_string();
        let mut prefix_to_module = HashMap::new();
        for imp in m.subs_of("import") {
            if let Some(p) = imp.sub("prefix") {
                prefix_to_module.insert(p.arg_str().to_string(), imp.arg_str().to_string());
            }
        }
        if let Some(p) = m.sub("prefix") {
            prefix_to_module.insert(p.arg_str().to_string(), mname.clone());
        }
        contexts.insert(mname.clone(), ModuleCtx { prefix_to_module });
    }

    let mut b = Builder {
        raw_modules,
        contexts,
        building: Default::default(),
        built: Default::default(),
        nodes: Vec::new(),
        typedefs: Vec::new(),
        identities: Vec::new(),
        identity_by_module_name: HashMap::new(),
        typedef_cache: HashMap::new(),
        module_root: HashMap::new(),
        yang_data: Vec::new(),
        structure_roots: HashMap::new(),
        implemented_yang_data: implemented_yang_data
            .iter()
            .map(|s| s.to_string())
            .collect(),
        identity_accept_lists: identity_accept_lists
            .iter()
            .map(|&(base, accepted)| {
                (
                    base.to_string(),
                    accepted.iter().map(|s| s.to_string()).collect(),
                )
            })
            .collect(),
        identityref_accept_lists: identityref_accept_lists
            .iter()
            .map(|&(leaf, accepted)| {
                (
                    leaf.to_string(),
                    accepted.iter().map(|s| s.to_string()).collect(),
                )
            })
            .collect(),
        reachable_typedefs: Default::default(),
        reachable_identities: Default::default(),
        implemented_identities: Default::default(),
        referenced_modules: Default::default(),
    };

    let module_names: Vec<String> = parsed.iter().map(|m| m.arg_str().to_string()).collect();
    for name in &module_names {
        b.ensure_module(name)?;
    }

    for sid_src in sid_sources {
        let f = sid::parse(sid_src).map_err(err)?;
        b.attach_sids(&f)?;
    }

    b.apply_deviations(&parsed)?;
    b.compute_reachable_typedefs()?;
    b.resolve_leafrefs()?;

    b.finish()
}

impl<'a> Builder<'a> {
    fn new_node(
        &mut self,
        kw: &str,
        name: String,
        owner_module: String,
        parent: Option<NodeId>,
        detached: bool,
        synthetic: bool,
    ) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(Node {
            kw: kw.to_string(),
            name,
            parent,
            children: Vec::new(),
            config: true,
            mandatory: false,
            ty: None,
            keys: Vec::new(),
            sid: None,
            owner_module: owner_module.clone(),
            description: None,
            status: None,
            default: None,
            synthetic,
            references_modules: Vec::new(),
        });
        if !detached {
            if let Some(p) = parent {
                self.nodes[p].children.push(id);
            } else {
                let module = self.module_root.get_mut(&owner_module).unwrap_or_else(|| {
                    panic!("module {owner_module:?} must exist before creating a node in it")
                });
                module.root_nodes.push(id);
            }
        }
        id
    }

    fn get_identity(&mut self, module: &str, name: &str) -> IdentityId {
        let key = (module.to_string(), name.to_string());
        if let Some(&id) = self.identity_by_module_name.get(&key) {
            return id;
        }
        let id = self.identities.len();
        self.identities.push(Identity {
            name: name.to_string(),
            module: module.to_string(),
            description: None,
            base: Vec::new(),
            derived: Vec::new(),
            sid: None,
        });
        self.identity_by_module_name.insert(key, id);
        id
    }

    /// Ensure `name`'s module is fully interpreted (recursively ensuring
    /// its imports first), memoized.
    fn ensure_module(&mut self, name: &str) -> R<()> {
        if self.built.contains(name) {
            return Ok(());
        }
        if self.building.contains(name) {
            return Ok(()); // import cycle guard; YANG forbids these anyway
        }
        self.building.insert(name.to_string());

        let raw = *self
            .raw_modules
            .get(name)
            .ok_or_else(|| err(format!("unknown module {name:?}")))?;
        let imports: Vec<String> = raw
            .subs_of("import")
            .map(|i| i.arg_str().to_string())
            .collect();
        for imp in &imports {
            self.ensure_module(imp)?;
        }

        // Register this module's identities up front (an identity's base
        // may be forward-referenced within the same module).
        for ident in raw.subs_of("identity") {
            let id = self.get_identity(name, ident.arg_str());
            self.identities[id].description =
                ident.sub("description").map(|d| d.arg_str().to_string());
            for base in ident.subs_of("base") {
                let base_id = self.resolve_identity_ref(name, base.arg_str())?;
                self.identities[id].base.push(base_id);
                self.identities[base_id].derived.push(id);
            }
        }

        let namespace = raw
            .sub("namespace")
            .ok_or_else(|| err(format!("module {name:?} has no namespace statement")))?
            .arg_str()
            .to_string();
        let revision = raw.sub("revision").map(|r| r.arg_str().to_string());

        let module = Module {
            name: name.to_string(),
            namespace,
            revision,
            sid: None,
            root_nodes: Vec::new(),
            deviated_by: Default::default(),
        };
        self.module_root.insert(name.to_string(), module);

        let ctx_name = name.to_string();
        self.interpret_scope_typedefs(&ctx_name, &ctx_name, &[], &raw.subs)?;
        self.interpret_children(&ctx_name, &ctx_name, &[], &raw.subs, None)?;
        let document_roots = self
            .structure_roots
            .values()
            .map(|&root| ("structure", root))
            .chain(
                self.yang_data
                    .iter()
                    .map(|y| ("yang-data container", y.root)),
            );
        for (kind, root) in document_roots {
            let document_root = &self.nodes[root];
            if document_root.owner_module == ctx_name
                && self
                    .find_in_module(&self.module_root[&ctx_name], document_root.local_name())
                    .is_some()
            {
                return Err(err(format!(
                    "{kind} {:?} in module {ctx_name:?} collides with a top-level data node of the same name",
                    document_root.local_name()
                )));
            }
        }

        self.built.insert(name.to_string());
        self.building.remove(name);
        Ok(())
    }

    fn resolve_identity_ref(&mut self, current_module: &str, value: &str) -> R<IdentityId> {
        let (module, local) = self.split_qualified(current_module, value)?;
        Ok(self.get_identity(&module, local))
    }

    /// Split a `prefix:name` (or bare `name`, defaulting to the current
    /// module) reference into its real module name and local part, using
    /// the current module's own import-prefix map.
    fn split_qualified<'v>(&self, current_module: &str, value: &'v str) -> R<(String, &'v str)> {
        match value.split_once(':') {
            Some((prefix, local)) => {
                let ctx = self
                    .contexts
                    .get(current_module)
                    .ok_or_else(|| err(format!("unknown module {current_module:?}")))?;
                let module = ctx
                    .resolve_prefix(prefix)
                    .ok_or_else(|| {
                        err(format!(
                            "unknown prefix {prefix:?} in module {current_module:?}"
                        ))
                    })?
                    .to_string();
                Ok((module, local))
            }
            None => Ok((current_module.to_string(), value)),
        }
    }

    /// Interpret a list of sibling statements (a module's top level, or a
    /// container/list/etc.'s substatements) under `parent`.
    ///
    /// Two separate module contexts are threaded through the whole
    /// interpret_* family, and they can diverge across nested `uses`:
    /// `resolve` is "whose source text (and hence whose import-prefix
    /// table) are we currently walking" -- it changes to a grouping's
    /// defining module while splicing that grouping's body, so
    /// type/typedef/identity/nested-grouping prefix lookups resolve
    /// correctly (RFC 7950 7.13). `naming` is "which module's namespace
    /// do newly created nodes belong to" for top-level qualification and
    /// augment cross-module detection -- per RFC 7950 7.13 a grouping's
    /// nodes belong to the *using* module's namespace even though its own
    /// source text resolves against its *defining* module, so `naming`
    /// stays constant across nested splices instead of following
    /// `resolve` down into each grouping.
    fn interpret_children(
        &mut self,
        resolve: &str,
        naming: &str,
        scope: &Scope<'a>,
        stmts: &'a [Stmt],
        parent: Option<NodeId>,
    ) -> R<()> {
        for s in stmts {
            self.interpret_one(resolve, naming, scope, s, parent)?;
        }
        Ok(())
    }

    fn interpret_one(
        &mut self,
        resolve: &str,
        naming: &str,
        scope: &Scope<'a>,
        s: &'a Stmt,
        parent: Option<NodeId>,
    ) -> R<()> {
        if s.prefix.is_some() {
            self.interpret_extension(resolve, naming, s, parent)?;
            return Ok(());
        }
        if SCHEMA_NODE_KEYWORDS.contains(&s.keyword.as_str()) {
            return self.interpret_schema_node(resolve, naming, scope, s, parent);
        }
        match s.keyword.as_str() {
            "uses" => self.interpret_uses(resolve, naming, scope, s, parent)?,
            "augment" => self.interpret_augment(resolve, naming, scope, s, parent, None)?,
            // Everything else here (typedef/grouping/identity/import,
            // module metadata, or a substatement like config/description
            // seen as a sibling rather than consumed by its owning
            // schema-node branch) produces no node of its own: registered
            // elsewhere, or silently skipped, matching yang-utils.rb's
            // handling of unrecognized/irrelevant statements.
            _ => {}
        }
        Ok(())
    }

    fn interpret_extension(
        &mut self,
        resolve: &str,
        naming: &str,
        s: &'a Stmt,
        parent: Option<NodeId>,
    ) -> R<()> {
        let ext_module = s.prefix.as_deref().and_then(|prefix| {
            self.contexts
                .get(resolve)
                .and_then(|ctx| ctx.resolve_prefix(prefix))
                .map(str::to_string)
        });
        if let (Some(p), Some(m)) = (parent, &ext_module) {
            self.nodes[p].references_modules.push(m.clone());
        }
        // RFC 8040 `rc:yang-data` (matches yang-utils.rb): must contain
        // exactly one container, promoted to a top-level root node named
        // "module:<container's own name>" --
        // NOT the yang-data statement's own argument, which is merely a
        // label (e.g. `rc:yang-data coreconf-error { container error {...`
        // promotes to "ietf-coreconf:error", matching the .sid file, not
        // "ietf-coreconf:coreconf-error"). Matches yang-utils.rb:255-256
        // (`container_stmt.arg.prepend(self.name, ':')`).
        if s.keyword == "yang-data" {
            if self
                .yang_data
                .iter()
                .any(|y| y.module == naming && y.name == s.arg_str())
            {
                return Err(err(format!(
                    "yang-data {:?} is defined more than once in module {naming:?}",
                    s.arg_str()
                )));
            }
            let scratch = self.new_node(
                "container",
                String::new(),
                naming.to_string(),
                None,
                true,
                true,
            );
            self.interpret_children(resolve, naming, &[], &s.subs, Some(scratch))?;
            let children = self.nodes[scratch].children.clone();
            let &[root] = children.as_slice() else {
                return Err(err(format!(
                    "yang-data {:?} must contain exactly one data node, found {}",
                    s.arg_str(),
                    children.len()
                )));
            };
            if self.nodes[root].kw != "container" {
                return Err(err(format!(
                    "yang-data {:?} must contain a container, found a {:?}",
                    s.arg_str(),
                    self.nodes[root].kw
                )));
            }
            let name = format!("{naming}:{}", self.nodes[root].name);
            if self.structure_roots.contains_key(&name)
                || self
                    .yang_data
                    .iter()
                    .any(|y| self.nodes[y.root].name == name)
            {
                return Err(err(format!(
                    "yang-data {:?}'s container {:?} in module {naming:?} collides with another yang-data container or structure of the same name",
                    s.arg_str(),
                    self.nodes[root].name
                )));
            }
            self.nodes[root].parent = None;
            self.nodes[root].name = name.clone();
            self.clear_config_in_subtree(root);
            if let Some(m) = &ext_module {
                self.nodes[root].references_modules.push(m.clone());
            }
            // `rc:yang-data` roots aren't children of their module's
            // node (they're "not part of the datastore" -- see
            // finish()'s comment -- so must stay out of module_root's
            // children, which finish() flattens into the real
            // datastore tree) but their own `.sid` file entries (e.g.
            // "/ietf-coreconf:error") still need to resolve to them,
            // since real error responses are addressed by exactly
            // these SIDs on the wire. Register separately so
            // resolve_sid_data_identifier can still find them.
            self.yang_data.push(YangData {
                module: naming.to_string(),
                name: s.arg_str().to_string(),
                root,
            });
        } else if s.keyword == "structure"
            && ext_module.as_deref() == Some("ietf-yang-structure-ext")
        {
            if parent.is_some() {
                return Err(err(format!(
                    "structure {:?} in module {naming:?} must be a top-level statement",
                    s.arg_str()
                )));
            }
            let name = format!("{naming}:{}", s.arg_str());
            if self.structure_roots.contains_key(&name) {
                return Err(err(format!(
                    "structure {:?} is defined more than once in module {naming:?}",
                    s.arg_str()
                )));
            }
            if self
                .yang_data
                .iter()
                .any(|y| self.nodes[y.root].name == name)
            {
                return Err(err(format!(
                    "structure {:?} in module {naming:?} collides with a yang-data container of the same name",
                    s.arg_str()
                )));
            }
            let root = self.new_node(
                "container",
                name.clone(),
                naming.to_string(),
                None,
                true,
                true,
            );
            self.nodes[root].status = Some(read_status(s)?);
            if let Some(d) = s.sub("description") {
                self.nodes[root].description = Some(d.arg_str().to_string());
            }
            let scope_label = self.schema_node_path_of(root);
            let scope = [(s, scope_label.clone())];
            self.interpret_scope_typedefs(resolve, &scope_label, &scope, &s.subs)?;
            self.interpret_children(resolve, naming, &scope, &s.subs, Some(root))?;
            self.clear_config_in_subtree(root);
            self.structure_roots.insert(name, root);
        }
        Ok(())
    }

    fn clear_config_in_subtree(&mut self, root: NodeId) {
        self.nodes[root].config = false;
        for child in self.nodes[root].children.clone() {
            self.clear_config_in_subtree(child);
        }
    }

    fn qualify_top_level(&self, module: &str, local: &str) -> String {
        format!("{module}:{local}")
    }

    fn interpret_schema_node(
        &mut self,
        resolve: &str,
        naming: &str,
        scope: &Scope<'a>,
        s: &'a Stmt,
        parent: Option<NodeId>,
    ) -> R<()> {
        let is_top = parent.is_none();
        // input/output are the only schema-node statements RFC 7950 gives
        // no argument to (there's always at most one of each per
        // rpc/action) -- use the keyword itself as the name.
        let local = match s.keyword.as_str() {
            "input" | "output" => s.keyword.as_str(),
            _ => s.arg_str(),
        };
        let name = if is_top {
            self.qualify_top_level(naming, local)
        } else {
            local.to_string()
        };

        let node = self.new_node(&s.keyword, name, naming.to_string(), parent, false, false);

        if let Some(cfg) = s.sub("config") {
            self.nodes[node].config = cfg.arg_str() == "true";
        } else if let Some(p) = self.nodes[node].parent {
            self.nodes[node].config = self.nodes[p].config;
        }
        if let Some(m) = s.sub("mandatory") {
            self.nodes[node].mandatory = m.arg_str() == "true";
        }
        if let Some(d) = s.sub("description") {
            self.nodes[node].description = Some(d.arg_str().to_string());
        }

        if s.keyword != "input" && s.keyword != "output" {
            self.nodes[node].status = Some(read_status(s)?);
        }

        if let Some(default) = s.sub("default") {
            self.nodes[node].default = Some(default.arg_str().to_string());
        }

        match s.keyword.as_str() {
            "leaf" | "leaf-list" => {
                if let Some(t) = s.sub("type") {
                    let ty = self.interpret_type(resolve, scope, t)?;
                    self.nodes[node].ty = Some(ty);
                }
            }
            "rpc" | "action" => {
                if s.sub("input").is_none() {
                    self.new_node(
                        "input",
                        "input".to_string(),
                        naming.to_string(),
                        Some(node),
                        false,
                        true,
                    );
                }
                if s.sub("output").is_none() {
                    self.new_node(
                        "output",
                        "output".to_string(),
                        naming.to_string(),
                        Some(node),
                        false,
                        true,
                    );
                }
            }
            _ => {}
        }

        const SCOPE_BEARING_KEYWORDS: &[&str] = &[
            "container",
            "list",
            "rpc",
            "action",
            "input",
            "output",
            "notification",
        ];
        let mut extended_scope = scope.to_vec();
        let inner_scope = if SCOPE_BEARING_KEYWORDS.contains(&s.keyword.as_str()) {
            let own_scope_label = self.schema_node_path_of(node);
            extended_scope.push((s, own_scope_label.clone()));
            self.interpret_scope_typedefs(
                resolve,
                &own_scope_label,
                extended_scope.as_slice(),
                &s.subs,
            )?;
            extended_scope.as_slice()
        } else {
            scope
        };

        if s.keyword == "choice" {
            self.interpret_choice_body(resolve, naming, inner_scope, &s.subs, node)?;
        } else {
            self.interpret_children(resolve, naming, inner_scope, &s.subs, Some(node))?;
        }

        if s.keyword == "list" {
            if let Some(k) = s.sub("key") {
                for local in k.arg_str().split_whitespace() {
                    let key_id = self.find_by_local(node, local).ok_or_else(|| {
                        err(format!(
                            "key {local:?} of list {:?} is not one of its own direct children",
                            self.nodes[node].name
                        ))
                    })?;
                    self.nodes[node].keys.push(key_id);
                }
            }
        }
        Ok(())
    }

    /// A choice's direct container/leaf/leaf-list/list/choice/anydata/
    /// anyxml child is wrapped in a synthetic, unnamed case sharing the
    /// child's own name (RFC 7950 7.9.2); an explicit `case` is used as
    /// declared.
    fn interpret_choice_body(
        &mut self,
        resolve: &str,
        naming: &str,
        scope: &Scope<'a>,
        stmts: &'a [Stmt],
        choice_node: NodeId,
    ) -> R<()> {
        const IMPLICIT_CASE_KEYWORDS: &[&str] = &[
            "container",
            "leaf",
            "leaf-list",
            "list",
            "choice",
            "anydata",
            "anyxml",
        ];
        for s in stmts {
            if s.prefix.is_none() && IMPLICIT_CASE_KEYWORDS.contains(&s.keyword.as_str()) {
                let case_id = self.new_node(
                    "case",
                    s.arg_str().to_string(),
                    naming.to_string(),
                    Some(choice_node),
                    false,
                    true,
                );
                self.interpret_one(resolve, naming, scope, s, Some(case_id))?;
                // Implicit case statements should have the same "status" as the data node it represents.
                let child_id = self.nodes[case_id]
                    .children
                    .first()
                    .expect("implicit case must have a child");
                self.nodes[case_id].status = self.nodes[*child_id].status;
            } else {
                self.interpret_one(resolve, naming, scope, s, Some(choice_node))?;
            }
        }
        Ok(())
    }

    fn interpret_uses(
        &mut self,
        resolve: &str,
        naming: &str,
        scope: &Scope<'a>,
        s: &'a Stmt,
        parent: Option<NodeId>,
    ) -> R<()> {
        let (def_module, local) = self.split_qualified(resolve, s.arg_str())?;
        let (raw, def_scope) = self.find_grouping_stmt(&def_module, scope, local)?;

        // Splice the grouping's body directly into the use site: its own
        // source text resolves against *its* defining module
        // (`def_module` becomes the new `resolve`), but the resulting
        // nodes still belong to the same using-module namespace as
        // before (`naming` passes through unchanged) -- see
        // `interpret_children`'s doc comment.
        let use_site_scope = match parent {
            Some(p) => self.schema_node_path_of(p),
            None => naming.to_string(),
        };
        let mut inner_scope = def_scope;
        inner_scope.push((raw, use_site_scope.clone()));
        self.interpret_scope_typedefs(&def_module, &use_site_scope, &inner_scope, &raw.subs)?;
        match parent {
            Some(p) => {
                self.interpret_children(&def_module, naming, &inner_scope, &raw.subs, parent)?;
                self.nodes[p].references_modules.push(def_module.clone());
            }
            None => {
                let module = &self.module_root[naming];
                let before = module.root_nodes.len();
                self.interpret_children(&def_module, naming, &inner_scope, &raw.subs, parent)?;
                let module = &self.module_root[naming];
                let new_roots: Vec<NodeId> = module.root_nodes[before..].to_vec();
                for root in new_roots {
                    self.nodes[root].references_modules.push(def_module.clone());
                }
            }
        }

        // An augment nested inside this `uses` targets a path within the
        // grouping's own body, which was just spliced in above. It's
        // lexically part of the *using* module's text (RFC 7950 7.13
        // treats the whole `uses` body as if copied there), so it
        // resolves and is named in `resolve`/`naming` unchanged, not
        // `def_module`.
        for aug in s.subs_of("augment") {
            self.interpret_augment(resolve, naming, scope, aug, parent, parent)?;
        }
        Ok(())
    }

    /// `relative_root`: `Some(parent)` for an augment nested in `uses`
    /// (relative to the use site); `None` for a module-level augment
    /// (absolute path from a module root). `resolve` locates the target
    /// (its own import-prefix table, for the absolute-path case);
    /// `naming` is compared against the target's owner to decide whether
    /// newly-added children need cross-module qualification.
    fn interpret_augment(
        &mut self,
        resolve: &str,
        naming: &str,
        scope: &Scope<'a>,
        s: &'a Stmt,
        _use_site: Option<NodeId>,
        relative_root: Option<NodeId>,
    ) -> R<()> {
        let path = s.arg_str();
        let target = match relative_root {
            Some(root) => self.resolve_relative_path(root, path)?,
            None => self.resolve_absolute_schema_node_id(resolve, path)?,
        };
        let target_owner = self.nodes[target].owner_module.clone();

        for child_stmt in &s.subs {
            // An augment body's legal contents are schema nodes and
            // `uses` (RFC 7950 7.17's data-def-stmt production) -- a
            // nested `augment` isn't legal directly here (only inside a
            // `uses`, handled separately by `interpret_uses`). Extension
            // statements and everything else (description/when/status/
            // ...) fall through `interpret_one`'s catch-all.
            if child_stmt.prefix.is_none()
                && !SCHEMA_NODE_KEYWORDS.contains(&child_stmt.keyword.as_str())
                && child_stmt.keyword != "uses"
            {
                continue;
            }
            let before = self.nodes[target].children.len();
            self.interpret_one(resolve, naming, scope, child_stmt, Some(target))?;
            // Namespace fixup (RFC 7950 4.2.8/7.17): a node augmented in
            // from a different module than the target's module is named
            // in the *augmenting* module's namespace.
            if naming != target_owner {
                let new_children: Vec<NodeId> = self.nodes[target].children[before..].to_vec();
                for new_child in new_children {
                    let local = self.nodes[new_child].local_name().to_string();
                    self.nodes[new_child].name = self.qualify_top_level(naming, &local);
                }
            }
        }
        Ok(())
    }

    fn resolve_relative_path(&self, root: NodeId, path: &str) -> R<NodeId> {
        let mut cur = root;
        for seg in path.split('/').filter(|s| !s.is_empty()) {
            let local = seg.rsplit_once(':').map(|(_, n)| n).unwrap_or(seg);
            cur = self.find_by_local(cur, local).ok_or_else(|| {
                err(format!(
                    "augment target segment {seg:?} not found (relative path {path:?})"
                ))
            })?;
        }
        Ok(cur)
    }

    /// Resolve a schema-node-id from a `.sid` file's `data`-namespace
    /// `identifier`. Unlike augment/deviation target-node-ids (which use
    /// the declaring module's own import-prefix *aliases*), a `.sid`
    /// file's first path segment prefix is always the real module name
    /// directly -- there's no per-file alias context to translate
    /// through. A real `.sid` file's own `data`-namespace identifier can
    /// name either an ordinary schema node or a `rc:yang-data` root
    /// (e.g. "/ietf-coreconf:error"), so both are tried.
    fn resolve_sid_data_identifier(&self, path: &str) -> R<NodeId> {
        resolve_schema_path(&self.nodes, &self.module_root, path)
            .or_else(|| {
                resolve_yang_data_path(&self.nodes, self.yang_data.iter().map(|y| y.root), path)
            })
            .or_else(|| {
                resolve_yang_data_path(&self.nodes, self.structure_roots.values().copied(), path)
            })
            .ok_or_else(|| {
                err(format!(
                    "target segment not found in SID data identifier {path:?}"
                ))
            })
    }

    fn find_in_module(&self, module: &Module, local: &str) -> Option<NodeId> {
        module
            .root_nodes
            .iter()
            .copied()
            .find(|&id| self.nodes[id].local_name() == local)
    }

    fn find_by_local(&self, parent: NodeId, local: &str) -> Option<NodeId> {
        self.nodes[parent]
            .children
            .iter()
            .copied()
            .find(|&c| self.nodes[c].local_name() == local)
    }

    /// Resolve an absolute schema-node-id (`/prefix:top/child/...`) using
    /// `current_module`'s own import-prefix map for the first segment.
    fn resolve_absolute_schema_node_id(&self, current_module: &str, path: &str) -> R<NodeId> {
        let mut segs = path.split('/').filter(|s| !s.is_empty());
        let first = segs
            .next()
            .ok_or_else(|| err(format!("empty schema-node-id {path:?}")))?;
        let (dst_module, first_local) = self.split_qualified(current_module, first)?;
        let module = self
            .module_root
            .get(&dst_module)
            .ok_or_else(|| err(format!("unknown target module {dst_module:?} in {path:?}")))?;
        let mut cur = self.find_in_module(module, first_local).ok_or_else(|| {
            err(format!(
                "target segment {first_local:?} not found in module {dst_module:?} ({path:?})"
            ))
        })?;
        for seg in segs {
            let local = seg.rsplit_once(':').map(|(_, n)| n).unwrap_or(seg);
            cur = self
                .find_by_local(cur, local)
                .ok_or_else(|| err(format!("target segment {seg:?} not found ({path:?})")))?;
        }
        Ok(cur)
    }

    fn schema_node_path_of(&self, id: NodeId) -> String {
        node_path_of(&self.nodes, id)
    }

    // -- types ---------------------------------------------------------

    fn interpret_scope_typedefs(
        &mut self,
        def_module: &str,
        typedef_scope: &str,
        scope: &Scope<'a>,
        stmts: &'a [Stmt],
    ) -> R<()> {
        let mut keys = Vec::new();
        for td in stmts.iter().filter(|s| s.keyword == "typedef") {
            let local = td.arg_str();
            let key = (typedef_scope.to_string(), local.to_string());
            if self.typedef_cache.contains_key(&key) {
                continue;
            }
            let description = td.sub("description").map(|d| d.arg_str().to_string());
            let id = self.typedefs.len();
            self.typedefs.push(Typedef {
                name: local.to_string(),
                module: def_module.to_string(),
                scope: typedef_scope.to_string(),
                description,
                ty: None,
            });
            self.typedef_cache.insert(
                key.clone(),
                (
                    id,
                    PendingTypedefType {
                        resolve: def_module.to_string(),
                        scope: scope.to_vec(),
                        stmt: td,
                    },
                ),
            );
            keys.push(key);
        }
        for key in keys {
            self.ensure_typedef_interpreted(&key)?;
        }
        Ok(())
    }

    fn find_grouping_stmt(
        &self,
        def_module: &str,
        scope: &Scope<'a>,
        local: &str,
    ) -> R<(&'a Stmt, ScopeVec<'a>)> {
        for i in (0..scope.len()).rev() {
            if let Some(g) = scope[i]
                .0
                .subs_of("grouping")
                .find(|g| g.arg_str() == local)
            {
                return Ok((g, scope[..=i].to_vec()));
            }
        }
        let module_stmt = *self
            .raw_modules
            .get(def_module)
            .ok_or_else(|| err(format!("unknown module {def_module:?}")))?;
        let g = module_stmt
            .subs_of("grouping")
            .find(|g| g.arg_str() == local)
            .ok_or_else(|| {
                err(format!(
                    "grouping {local:?} not found in module {def_module:?}"
                ))
            })?;
        Ok((g, vec![]))
    }

    fn resolve_typedef(
        &mut self,
        def_module: &str,
        scope: &Scope<'a>,
        local: &str,
    ) -> R<TypedefId> {
        let mut typedef_scope = None;
        for i in (0..scope.len()).rev() {
            let key = (scope[i].1.clone(), local.to_string());
            if self.typedef_cache.contains_key(&key) {
                typedef_scope = Some(scope[i].1.clone());
                break;
            }
        }
        let typedef_scope = typedef_scope.unwrap_or_else(|| def_module.to_string());
        let key = (typedef_scope, local.to_string());
        self.ensure_typedef_interpreted(&key)?;
        let (id, _) = self.typedef_cache.get(&key).ok_or_else(|| {
            err(format!(
                "typedef {local:?} not found in module {def_module:?}"
            ))
        })?;
        Ok(*id)
    }

    fn ensure_typedef_interpreted(&mut self, key: &(String, String)) -> R<()> {
        let Some((id, pending)) = self.typedef_cache.get(key) else {
            return Ok(());
        };
        let id = *id;
        if self.typedefs[id].ty.is_some() {
            return Ok(());
        }
        let (def_module, scope, stmt) =
            (pending.resolve.clone(), pending.scope.clone(), pending.stmt);
        let base = stmt
            .sub("type")
            .ok_or_else(|| err(format!("typedef {:?} has no type", self.typedefs[id].name)))?;
        let ty = self.interpret_type(&def_module, &scope, base)?;
        self.typedefs[id].ty = Some(ty);
        Ok(())
    }

    fn interpret_type(&mut self, module: &str, scope: &Scope<'a>, t: &Stmt) -> R<TypeDescriptor> {
        let name = t.arg_str();
        let mut ty = if let Some(builtin) = Builtin::from_name(name) {
            self.interpret_builtin_type(module, scope, builtin, t)?
        } else {
            // typedef reference: clone the named typedef's own
            // descriptor as the starting point for this use site's own
            // restrictions below -- a clone, not a shared reference,
            // since this same typedef can be used (and independently
            // further restricted) at more than one site. Mirrors
            // `inner.derive(typedef['name'])` (yang-utils.rb:324).
            let (def_module, local) = self.split_qualified(module, name)?;
            let typedef_id = if def_module == module {
                self.resolve_typedef(&def_module, scope, local)?
            } else {
                self.resolve_typedef(&def_module, &[], local)?
            };
            let mut ty = self.typedefs[typedef_id]
                .ty
                .clone()
                .expect("resolve_typedef always ensures its own type is interpreted first");
            ty.base = Some(typedef_id);
            ty
        };

        // Apply this type statement's own range/length/pattern
        // restrictions on top of whatever it started with (builtin
        // defaults, or a typedef's already-narrowed ranges/patterns) --
        // mirrors yang-utils.rb's `interpret_type` applying `pattern`/
        // `range`/`length` unconditionally after resolving `t`,
        // regardless of whether `t` was a builtin or typedef reference.
        if !ty.ranges.is_empty() {
            if let Some(range) = t.sub("range") {
                ty.ranges = restrict_range(&ty.ranges, range.arg_str());
            }
            if let Some(length) = t.sub("length") {
                ty.ranges = restrict_range(&ty.ranges, length.arg_str());
            }
        }
        for p in t.subs_of("pattern") {
            ty.patterns.push(p.arg_str().to_string());
        }

        Ok(ty)
    }

    fn interpret_builtin_type(
        &mut self,
        module: &str,
        scope: &Scope<'a>,
        builtin: Builtin,
        t: &Stmt,
    ) -> R<TypeDescriptor> {
        let mut ty = TypeDescriptor::new(builtin);
        ty.ranges = default_ranges(builtin);

        match builtin {
            Builtin::Decimal64 => {
                if let Some(fd) = t.sub("fraction-digits") {
                    ty.fraction_digits = fd.arg_str().parse().ok();
                }
            }
            Builtin::Bits => {
                let mut next_pos = 0u32;
                for b in t.subs_of("bit") {
                    let position = b
                        .sub("position")
                        .and_then(|p| p.arg_str().parse().ok())
                        .unwrap_or(next_pos);
                    next_pos = position + 1;
                    ty.bits.push(Bit {
                        name: b.arg_str().to_string(),
                        position,
                    });
                }
            }
            Builtin::Enumeration => {
                let mut next_val = 0i64;
                for e in t.subs_of("enum") {
                    let value = e
                        .sub("value")
                        .and_then(|v| v.arg_str().parse().ok())
                        .unwrap_or(next_val);
                    next_val = value + 1;
                    ty.enums.push(EnumVal {
                        name: e.arg_str().to_string(),
                        value,
                    });
                }
            }
            Builtin::Identityref => {
                ty.source_module = Some(module.to_string());
                for b in t.subs_of("base") {
                    let id = self.resolve_identity_ref(module, b.arg_str())?;
                    ty.identity_bases.push(id);
                }
            }
            Builtin::Leafref => {
                if let Some(p) = t.sub("path") {
                    ty.leafref_path = Some(p.arg_str().to_string());
                    ty.source_module = Some(module.to_string());
                }
            }
            Builtin::Union => {
                for m in t.subs_of("type") {
                    let member = self.interpret_type(module, scope, m)?;
                    ty.union_members.push(member);
                }
            }
            _ => {}
        }

        Ok(ty)
    }

    // -- deviation -------------------------------------------------------

    fn apply_deviations(&mut self, modules: &[Stmt]) -> R<()> {
        for m in modules {
            let module = m.arg_str();
            for dev in m.subs_of("deviation") {
                let target = self.resolve_absolute_schema_node_id(module, dev.arg_str())?;
                let target_module_name = self.nodes[target].owner_module.clone();
                if target_module_name != module {
                    let target_module = self
                        .module_root
                        .get_mut(&target_module_name)
                        .expect("module exists if its target node does");
                    target_module.deviated_by.insert(module.to_string());
                }
                for d in dev.subs_of("deviate") {
                    match d.arg_str() {
                        "not-supported" => self.remove_node(target),
                        "replace" => {
                            if let Some(cfg) = d.sub("config") {
                                let cfg_val = cfg.arg_str() == "true";
                                self.nodes[target].config = cfg_val;
                                if !cfg_val {
                                    self.propagate_config_false(target);
                                }
                            }
                            if let Some(t) = d.sub("type") {
                                let ty = self.interpret_type(module, &[], t)?;
                                self.nodes[target].ty = Some(ty);
                            }
                        }
                        "add" | "delete" => {
                            // Unimplemented, matching yang-utils.rb (real
                            // catalogs in this repo don't use these).
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    }

    fn remove_node(&mut self, target: NodeId) {
        let parent = self.nodes[target].parent;

        if let Some(p) = parent {
            self.nodes[p].children.retain(|&c| c != target);
        } else {
            let owner_module = self.nodes[target].owner_module.clone();
            if let Some(module) = self.module_root.get_mut(&owner_module) {
                module.root_nodes.retain(|&c| c != target);
            }
        }
    }

    fn propagate_config_false(&mut self, node: NodeId) {
        let len = self.nodes[node].children.len();
        for i in 0..len {
            let c = self.nodes[node].children[i];
            self.nodes[c].config = false;
            self.propagate_config_false(c);
        }
    }

    // -- leafref ---------------------------------------------------------

    fn resolve_leafrefs(&mut self) -> R<()> {
        let mut reachable = vec![false; self.nodes.len()];
        let roots: Vec<NodeId> = self
            .module_root
            .values()
            .flat_map(|m| m.root_nodes.iter().copied())
            .chain(self.yang_data.iter().map(|y| y.root))
            .collect();
        for root in roots {
            self.mark_reachable(root, &mut reachable);
        }

        for (id, &is_reachable) in reachable.iter().enumerate() {
            if !is_reachable {
                continue;
            }
            if let Some(mut ty) = self.nodes[id].ty.take() {
                self.resolve_leafref_in_descriptor(id, &mut ty)?;
                self.nodes[id].ty = Some(ty);
            }
        }
        Ok(())
    }

    fn mark_reachable(&self, node: NodeId, reachable: &mut [bool]) {
        reachable[node] = true;
        for &c in &self.nodes[node].children {
            self.mark_reachable(c, reachable);
        }
    }

    fn resolve_leafref_in_descriptor(&self, node: NodeId, ty: &mut TypeDescriptor) -> R<()> {
        if ty.builtin == Builtin::Leafref {
            if let Some(path) = ty.leafref_path.clone() {
                let module = ty
                    .source_module
                    .clone()
                    .expect("a leafref's source_module is always set alongside leafref_path");
                let target = if let Some(abs) = path.trim().strip_prefix('/') {
                    self.resolve_absolute_schema_node_id(&module, &format!("/{abs}"))?
                } else {
                    self.resolve_relative_leafref(node, &path).ok_or_else(|| {
                        err(format!(
                            "leafref path {path:?} (declared in module {module:?}) could not be resolved"
                        ))
                    })?
                };
                let target_kw = &self.nodes[target].kw;
                if target_kw != "leaf" && target_kw != "leaf-list" {
                    return Err(err(format!(
                        "leafref path {path:?} (declared in module {module:?}) refers to a {target_kw}, not a leaf or leaf-list"
                    )));
                }
                ty.leafref_target = Some(target);
            }
        }
        for member in &mut ty.union_members {
            self.resolve_leafref_in_descriptor(node, member)?;
        }
        Ok(())
    }

    /// A relative leafref path (RFC 7950 9.9.2: `1*(".." "/") descendant-
    /// path`) is anchored at the leaf's own *parent* -- each `..` climbs
    /// one more level before the remaining segments descend by name.
    fn resolve_relative_leafref(&self, node: NodeId, path: &str) -> Option<NodeId> {
        // RFC 7950 9.9.2: the context node is the leafref leaf/leaf-list
        // itself, and each ".." climbs one level *from* there -- not
        // from its parent.
        let mut cur = node;
        for seg in path.trim().split('/').filter(|s| !s.is_empty()) {
            if seg == ".." {
                cur = self.nodes[cur].parent?;
            } else {
                let local = seg.rsplit_once(':').map(|(_, n)| n).unwrap_or(seg);
                cur = self.find_by_local(cur, local)?;
            }
        }
        Some(cur)
    }

    // -- SID attachment ---------------------------------------------------

    fn attach_sids(&mut self, f: &sid::SidFile) -> R<()> {
        for item in &f.items {
            match item.namespace {
                Namespace::Module => {
                    if let Some(module) = self.module_root.get_mut(&item.identifier) {
                        module.sid = Some(item.sid);
                    }
                }
                Namespace::Identity => {
                    if let Some(&id) = self
                        .identity_by_module_name
                        .get(&(f.module_name.clone(), item.identifier.clone()))
                    {
                        self.identities[id].sid = Some(item.sid);
                    }
                }
                Namespace::Data => {
                    let target = self.resolve_sid_data_identifier(&item.identifier)?;
                    self.nodes[target].sid = Some(item.sid);
                }
                Namespace::Feature => {}
            }
        }
        Ok(())
    }

    fn identity_by_qualified_name(&self, name: &str, what: &str) -> R<Option<IdentityId>> {
        let (module, local) = name.split_once(':').ok_or_else(|| {
            err(format!(
                "{what} {name:?} must be a qualified \"module:name\""
            ))
        })?;
        Ok(self
            .identity_by_module_name
            .get(&(module.to_string(), local.to_string()))
            .copied())
    }

    fn accepted_identities(
        &self,
        accepted_names: &[String],
        bases: &[IdentityId],
        owner: &str,
    ) -> R<std::collections::HashSet<IdentityId>> {
        let derived = derived_from_all(&self.identities, bases);
        let mut accepted = std::collections::HashSet::new();
        for accepted_name in accepted_names {
            let id = self
                .identity_by_qualified_name(accepted_name, "identity accept-list entry")?
                .ok_or_else(|| {
                    err(format!(
                        "identity accept-list entry {accepted_name:?} (accepted by {owner}) does not name a real identity"
                    ))
                })?;
            if !derived.contains(&id) {
                return Err(err(format!(
                    "identity accept-list entry {accepted_name:?} is not actually derived from {owner}"
                )));
            }
            accepted.insert(id);
        }
        Ok(accepted)
    }

    fn accept_lists_by_base(
        &self,
    ) -> R<HashMap<IdentityId, std::collections::HashSet<IdentityId>>> {
        let mut out = HashMap::new();
        for (base_name, accepted_names) in &self.identity_accept_lists {
            let Some(base_id) =
                self.identity_by_qualified_name(base_name, "identity accept-list base")?
            else {
                continue;
            };
            let accepted = self.accepted_identities(
                accepted_names,
                &[base_id],
                &format!("base {base_name:?}"),
            )?;
            out.insert(base_id, accepted);
        }
        Ok(out)
    }

    fn accept_lists_by_leaf(&self) -> R<HashMap<NodeId, std::collections::HashSet<IdentityId>>> {
        let mut out = HashMap::new();
        for (leaf, accepted_names) in &self.identityref_accept_lists {
            let node = resolve_schema_path(&self.nodes, &self.module_root, leaf)
                .or_else(|| {
                    resolve_yang_data_path(&self.nodes, self.yang_data.iter().map(|y| y.root), leaf)
                })
                .ok_or_else(|| {
                    err(format!(
                        "identityref accept-list leaf {leaf:?} does not name an implemented schema node"
                    ))
                })?;
            let kw = &self.nodes[node].kw;
            if kw != "leaf" && kw != "leaf-list" {
                return Err(err(format!(
                    "identityref accept-list leaf {leaf:?} names a {kw}, not a leaf or leaf-list"
                )));
            }
            let ty = self.nodes[node]
                .ty
                .as_ref()
                .expect("a leaf or leaf-list always has a type");
            if ty.builtin != Builtin::Identityref {
                return Err(err(format!(
                    "identityref accept-list leaf {leaf:?} is not an identityref, so its values can't be narrowed per leaf"
                )));
            }
            let accepted = self.accepted_identities(
                accepted_names,
                &ty.identity_bases,
                &format!("leaf {leaf:?}'s base"),
            )?;
            if out.insert(node, accepted).is_some() {
                return Err(err(format!(
                    "identityref accept-list leaf {leaf:?} is listed more than once"
                )));
            }
        }
        Ok(out)
    }

    fn assign_identity_values(
        &self,
        ty: &mut TypeDescriptor,
        by_leaf: Option<&std::collections::HashSet<IdentityId>>,
        by_base: &HashMap<IdentityId, std::collections::HashSet<IdentityId>>,
    ) {
        if ty.builtin == Builtin::Identityref {
            let values = match by_leaf {
                Some(accepted) => accepted.clone(),
                None => {
                    let mut values: Option<std::collections::HashSet<IdentityId>> = None;
                    for base in &ty.identity_bases {
                        let for_base = match by_base.get(base) {
                            Some(accepted) => accepted.clone(),
                            None => transitive_derived_excluding_self(&self.identities, *base),
                        };
                        values = Some(match values {
                            Some(v) => v.intersection(&for_base).copied().collect(),
                            None => for_base,
                        });
                    }
                    values.unwrap_or_default()
                }
            };
            ty.identity_values = Some(values);
        }
        for member in &mut ty.union_members {
            self.assign_identity_values(member, None, by_base);
        }
    }

    fn compute_reachable_typedefs(&mut self) -> R<()> {
        let by_base = self.accept_lists_by_base()?;

        for name in &self.implemented_yang_data {
            if !self.yang_data.iter().any(|y| y.qualified_name() == *name) {
                return Err(err(format!(
                    "implemented yang-data {name:?} does not name a real yang-data template"
                )));
            }
        }
        self.yang_data
            .retain(|y| self.implemented_yang_data.contains(&y.qualified_name()));
        let by_leaf = self.accept_lists_by_leaf()?;

        let mut reachable_typedefs = std::collections::HashSet::new();
        let mut identity_base_seeds = std::collections::HashSet::new();
        let mut referenced_modules = std::collections::HashSet::new();
        let all_root_nodes: Vec<NodeId> = self
            .module_root
            .values()
            .flat_map(|m| m.root_nodes.iter().copied())
            .chain(self.yang_data.iter().map(|y| y.root))
            .collect();
        for root in all_root_nodes {
            self.mark_reachable_typedefs_in_subtree(
                root,
                &mut reachable_typedefs,
                &mut identity_base_seeds,
                &mut referenced_modules,
            );
        }
        self.reachable_typedefs = reachable_typedefs;
        self.referenced_modules = referenced_modules;

        let mut live_nodes = Vec::new();
        let mut stack: Vec<NodeId> = self
            .module_root
            .values()
            .flat_map(|m| m.root_nodes.iter().copied())
            .chain(self.yang_data.iter().map(|y| y.root))
            .collect();
        while let Some(id) = stack.pop() {
            live_nodes.push(id);
            stack.extend(self.nodes[id].children.iter().copied());
        }
        let mut implemented_identities = std::collections::HashSet::new();
        for id in live_nodes {
            if let Some(mut ty) = self.nodes[id].ty.take() {
                self.assign_identity_values(&mut ty, by_leaf.get(&id), &by_base);
                collect_identity_values(&ty, &mut implemented_identities);
                self.nodes[id].ty = Some(ty);
            }
        }

        let mut reachable_identities = std::collections::HashSet::new();
        let mut stack: Vec<IdentityId> = implemented_identities
            .iter()
            .chain(identity_base_seeds.iter())
            .copied()
            .collect();
        while let Some(id) = stack.pop() {
            if reachable_identities.insert(id) {
                stack.extend(self.identities[id].base.iter().copied());
            }
        }

        self.implemented_identities = implemented_identities;
        self.reachable_identities = reachable_identities;
        Ok(())
    }

    fn finish(mut self) -> R<Schema> {
        let yang_data_owners: std::collections::HashSet<&str> =
            self.yang_data.iter().map(|y| y.module.as_str()).collect();

        let implemented_identity_owners: std::collections::HashSet<&str> = self
            .implemented_identities
            .iter()
            .map(|&id| self.identities[id].module.as_str())
            .collect();

        let implemented_names: std::collections::HashSet<String> = self
            .raw_modules
            .iter()
            .filter(|(name, raw)| {
                let module = &self.module_root[name.as_str()];
                let has_root_nodes = !module.root_nodes.is_empty();
                let has_surviving_augment = raw.subs_of("augment").any(|aug| {
                    self.resolve_absolute_schema_node_id(name, aug.arg_str())
                        .is_ok()
                });
                let has_deviation = raw.sub("deviation").is_some();
                let is_deviated = !module.deviated_by.is_empty();
                let has_yang_data = yang_data_owners.contains(name.as_str());
                let has_implemented_identity = implemented_identity_owners.contains(name.as_str());
                has_root_nodes
                    || has_surviving_augment
                    || has_deviation
                    || is_deviated
                    || has_yang_data
                    || has_implemented_identity
            })
            .map(|(name, _)| name.clone())
            .collect();

        let mut used_names: std::collections::HashSet<String> = self
            .reachable_typedefs
            .iter()
            .map(|&id| self.typedefs[id].module.clone())
            .chain(
                self.reachable_identities
                    .iter()
                    .map(|&id| self.identities[id].module.clone()),
            )
            .chain(self.referenced_modules.iter().cloned())
            .collect();
        used_names.retain(|name| !implemented_names.contains(name));

        let graph_names: std::collections::HashSet<&str> = implemented_names
            .iter()
            .map(String::as_str)
            .chain(used_names.iter().map(String::as_str))
            .collect();
        let mut import_only_modules: Vec<ImportOnlyModule> = Vec::new();
        for name in &used_names {
            let module = &self.module_root[name];
            let mut revision_dates: std::collections::HashSet<Option<String>> =
                std::collections::HashSet::new();
            for &importer in &graph_names {
                let raw = self.raw_modules[importer];
                for imp in raw.subs_of("import") {
                    if imp.arg_str() == name.as_str() {
                        revision_dates
                            .insert(imp.sub("revision-date").map(|r| r.arg_str().to_string()));
                    }
                }
            }
            if revision_dates.is_empty() {
                revision_dates.insert(None);
            }
            for revision_date in revision_dates {
                import_only_modules.push(ImportOnlyModule {
                    name: module.name.clone(),
                    namespace: module.namespace.clone(),
                    import_revision_date: revision_date,
                    sid: module.sid,
                });
            }
        }
        self.module_root
            .retain(|name, _| implemented_names.contains(name));

        let mut sid_index = HashMap::new();
        let all_root_nodes: Vec<NodeId> = self
            .module_root
            .values()
            .flat_map(|m| m.root_nodes.iter().copied())
            .collect();
        for root in all_root_nodes {
            self.index_sids(root, &mut sid_index);
        }
        // `rc:yang-data` roots (e.g. `ietf-coreconf:error`) are
        // deliberately not any module's root_nodes (see
        // interpret_extension) but their SIDs are still real wire values
        // (a device error response is addressed by exactly these SIDs),
        // so index each one's subtree separately.
        let yang_data_roots: Vec<NodeId> = self.yang_data.iter().map(|y| y.root).collect();
        for yd_root in yang_data_roots {
            self.index_sids(yd_root, &mut sid_index);
        }

        Ok(Schema {
            nodes: self.nodes,
            typedefs: self.typedefs,
            identities: self.identities,
            module_root: self.module_root,
            import_only_modules,
            yang_data: self.yang_data,
            structure_roots: self.structure_roots,
            sid_index,
            reachable_typedefs: self.reachable_typedefs,
            reachable_identities: self.reachable_identities,
            implemented_identities: self.implemented_identities,
        })
    }

    fn mark_reachable_typedefs_in_subtree(
        &self,
        node: NodeId,
        out: &mut std::collections::HashSet<TypedefId>,
        identity_base_seeds: &mut std::collections::HashSet<IdentityId>,
        referenced_modules: &mut std::collections::HashSet<String>,
    ) {
        if let Some(ty) = &self.nodes[node].ty {
            self.mark_reachable_typedefs(ty, out, identity_base_seeds);
        }
        referenced_modules.extend(self.nodes[node].references_modules.iter().cloned());
        for &c in &self.nodes[node].children {
            self.mark_reachable_typedefs_in_subtree(
                c,
                out,
                identity_base_seeds,
                referenced_modules,
            );
        }
    }

    fn mark_reachable_typedefs(
        &self,
        ty: &TypeDescriptor,
        out: &mut std::collections::HashSet<TypedefId>,
        identity_base_seeds: &mut std::collections::HashSet<IdentityId>,
    ) {
        if let Some(base) = ty.base {
            if out.insert(base) {
                let base_ty = self.typedefs[base]
                    .ty
                    .as_ref()
                    .expect("every typedef's own type is interpreted before reachability runs");
                self.mark_reachable_typedefs(base_ty, out, identity_base_seeds);
            }
        }
        identity_base_seeds.extend(ty.identity_bases.iter().copied());
        for member in &ty.union_members {
            self.mark_reachable_typedefs(member, out, identity_base_seeds);
        }
    }

    fn index_sids(&self, node: NodeId, out: &mut HashMap<i64, NodeId>) {
        if let Some(sid) = self.nodes[node].sid {
            out.insert(sid, node);
        }
        for &c in &self.nodes[node].children {
            self.index_sids(c, out);
        }
    }
}
