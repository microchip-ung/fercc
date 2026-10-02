// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

//! Semantic interpretation of parsed YANG statement trees into a schema
//! tree with attached SIDs: grouping/uses instantiation, augment, typedef
//! resolution, identity graph, leafref resolution.
//!
//! Ported from `support/yang-enc/yang-utils.rb`'s `Yang::ModuleSet` /
//! `Module` / `Statement` / `Type` / `Identity` classes -- same feature
//! set, not more (no `when`/`must`, only the `yang-data` extension,
//! `deviate add/delete` unimplemented), cross-checked structurally against
//! `sw-velocitydrive-devclient`'s `src/yang/yang-utils.ts`.
//!
//! Arena-based (`Vec<Node>` etc. plus `NodeId` indices) rather than an
//! object graph with parent pointers, which is the natural shape in Rust.

use std::collections::HashMap;

use crate::parser::{self, Stmt};
use crate::sid::{self, Namespace};

type NodeId = usize;
type IdentityId = usize;
type TypedefId = usize;

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
        }
    }
}

#[derive(Debug, Clone)]
pub struct Typedef {
    pub name: String,
    pub module: String,
    pub description: Option<String>,
    ty: TypeDescriptor,
}

/// The natural bounds a builtin type starts with before any `range`/
/// `length` statement narrows them, mirroring `Type#initialize`
/// (yang-utils.rb:654-661). Every other builtin starts with no ranges at
/// all -- there is nothing to narrow and nothing downstream reads them.
fn default_ranges(builtin: Builtin) -> Vec<(i128, i128)> {
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
    base: Vec<IdentityId>,
    derived: Vec<IdentityId>,
    pub sid: Option<i64>,
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
    /// List keys, in schema-declared order (not necessarily input order).
    pub keys: Vec<String>,
    pub sid: Option<i64>,
    pub owner_module: String,
    /// This node's own `description` substatement text, if any --
    /// tracked only for [`crate::json_schema`]'s benefit (matches
    /// `get_description`, yang-utils.rb:549-555: the first `description`
    /// substatement's text, verbatim, no further processing).
    pub description: Option<String>,
    pub status: Option<Status>,
    pub default: Option<String>,
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
    pub sid: Option<i64>,
    root_nodes: Vec<NodeId>,
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
    yang_data_roots: HashMap<String, NodeId>,
    sid_index: HashMap<i64, NodeId>,
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
            ty: &self.schema.typedefs[self.id].ty,
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

    pub fn derived(&self) -> impl Iterator<Item = IdentityRef<'a>> + 'a {
        let schema = self.schema;
        schema.identities[self.id]
            .derived
            .iter()
            .map(move |&id| IdentityRef { schema, id })
    }

    /// All identities transitively derived from this one (including
    /// itself), mirroring `Identity#derived_from`.
    pub fn derived_from(&self) -> std::collections::HashSet<IdentityRef<'a>> {
        let mut out = std::collections::HashSet::new();
        let mut stack = vec![*self];
        while let Some(id) = stack.pop() {
            if out.insert(id) {
                stack.extend(id.derived());
            }
        }
        out
    }
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

fn find_child_by_local_raw(nodes: &[Node], parent: NodeId, local: &str) -> Option<NodeId> {
    nodes[parent]
        .children
        .iter()
        .copied()
        .find(|&c| nodes[c].local_name() == local)
}

fn find_data_child_by_local_raw(nodes: &[Node], parent: NodeId, local: &str) -> Option<NodeId> {
    spliced(nodes, &nodes[parent].children)
        .into_iter()
        .find(|&c| nodes[c].local_name() == local)
}

fn descend_by_local<'s>(
    nodes: &[Node],
    start: NodeId,
    segs: impl Iterator<Item = &'s str>,
) -> Option<NodeId> {
    let mut cur = start;
    for seg in segs {
        let local = seg.rsplit_once(':').map(|(_, n)| n).unwrap_or(seg);
        cur = find_child_by_local_raw(nodes, cur, local)?;
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
        let local = seg.rsplit_once(':').map(|(_, n)| n).unwrap_or(seg);
        cur = find_data_child_by_local_raw(nodes, cur, local)?;
    }
    Some(cur)
}

fn resolve_schema_path(
    nodes: &[Node],
    module_root: &HashMap<String, Module>,
    yang_data_roots: &HashMap<String, NodeId>,
    path: &str,
) -> Option<NodeId> {
    let mut segs = path.split('/').filter(|s| !s.is_empty());
    let first = segs.next()?;
    let first_node = if let Some(&root) = yang_data_roots.get(first) {
        root
    } else {
        let (module_name, first_local) = first.split_once(':')?;
        let module = module_root.get(module_name)?;
        module
            .root_nodes
            .iter()
            .copied()
            .find(|&id| nodes[id].local_name() == first_local)?
    };
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
        self.identities
            .iter()
            .position(|i| i.module == module && i.name == name)
            .map(|id| IdentityRef { schema: self, id })
    }

    pub fn typedef_by_name(&self, module: &str, name: &str) -> Option<TypedefRef<'_>> {
        self.typedefs
            .iter()
            .position(|t| t.module == module && t.name == name)
            .map(|id| TypedefRef { schema: self, id })
    }

    pub fn typedefs(&self) -> impl Iterator<Item = TypedefRef<'_>> + '_ {
        (0..self.typedefs.len()).map(|id| TypedefRef { schema: self, id })
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

    pub fn node_by_schema_path(&self, path: &str) -> Option<NodeRef<'_>> {
        let id = resolve_schema_path(&self.nodes, &self.module_root, &self.yang_data_roots, path)?;
        Some(self.node_ref(id))
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

/// A named grouping or typedef, resolvable by (module, name) with lexical
/// scoping (nearer/inner definitions shadow outer ones is not modeled --
/// real catalogs don't rely on shadowing; module-top-level and any
/// ancestor's siblings are all visible).
#[derive(Clone)]
struct Definitions<'a> {
    typedefs: HashMap<(String, String), &'a Stmt>,
    groupings: HashMap<(String, String), &'a Stmt>,
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
    defs: Definitions<'a>,
    building: std::collections::HashSet<String>,
    built: std::collections::HashSet<String>,

    nodes: Vec<Node>,
    typedefs: Vec<Typedef>,
    identities: Vec<Identity>,
    identity_by_module_name: HashMap<(String, String), IdentityId>,
    typedef_cache: HashMap<(String, String), TypedefId>,
    module_root: HashMap<String, Module>,
    yang_data_roots: HashMap<String, NodeId>,
}

/// Parse `.yang` sources and build the full schema, including flattening
/// and SID attachment from `.sid` sources. Fresh every call -- no
/// caching, by design.
pub fn build(yang_sources: &[String], sid_sources: &[String]) -> R<Schema> {
    let parsed: Vec<Stmt> = yang_sources
        .iter()
        .map(|s| parser::parse_module(s).map_err(|e| err(e.to_string())))
        .collect::<R<Vec<_>>>()?;

    let mut raw_modules = HashMap::new();
    for m in &parsed {
        raw_modules.insert(m.arg_str().to_string(), m);
    }

    let mut contexts = HashMap::new();
    let mut typedefs = HashMap::new();
    let mut groupings = HashMap::new();
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

        for td in m.subs_of("typedef") {
            typedefs.insert((mname.clone(), td.arg_str().to_string()), td);
        }
        for g in m.subs_of("grouping") {
            groupings.insert((mname.clone(), g.arg_str().to_string()), g);
        }
    }

    let mut b = Builder {
        raw_modules,
        contexts,
        defs: Definitions {
            typedefs,
            groupings,
        },
        building: Default::default(),
        built: Default::default(),
        nodes: Vec::new(),
        typedefs: Vec::new(),
        identities: Vec::new(),
        identity_by_module_name: HashMap::new(),
        typedef_cache: HashMap::new(),
        module_root: HashMap::new(),
        yang_data_roots: HashMap::new(),
    };

    let module_names: Vec<String> = parsed.iter().map(|m| m.arg_str().to_string()).collect();
    for name in &module_names {
        b.ensure_module(name)?;
    }

    b.apply_deviations(&parsed)?;
    b.resolve_leafrefs()?;

    for sid_src in sid_sources {
        let f = sid::parse(sid_src).map_err(err)?;
        b.attach_sids(&f)?;
    }

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
            for base in ident.subs_of("base") {
                let base_id = self.resolve_identity_ref(name, base.arg_str())?;
                self.identities[id].base.push(base_id);
                self.identities[base_id].derived.push(id);
            }
        }

        let module = Module {
            name: name.to_string(),
            sid: None,
            root_nodes: Vec::new(),
        };
        self.module_root.insert(name.to_string(), module);

        let ctx_name = name.to_string();
        self.interpret_children(&ctx_name, &ctx_name, &raw.subs, None)?;

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
        stmts: &[Stmt],
        parent: Option<NodeId>,
    ) -> R<()> {
        for s in stmts {
            self.interpret_one(resolve, naming, s, parent)?;
        }
        Ok(())
    }

    fn interpret_one(
        &mut self,
        resolve: &str,
        naming: &str,
        s: &Stmt,
        parent: Option<NodeId>,
    ) -> R<()> {
        if s.prefix.is_some() {
            self.interpret_extension(naming, s, parent)?;
            return Ok(());
        }
        if SCHEMA_NODE_KEYWORDS.contains(&s.keyword.as_str()) {
            return self.interpret_schema_node(resolve, naming, s, parent);
        }
        match s.keyword.as_str() {
            "uses" => self.interpret_uses(resolve, naming, s, parent)?,
            "augment" => self.interpret_augment(resolve, naming, s, parent, None)?,
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

    fn interpret_extension(&mut self, naming: &str, s: &Stmt, _parent: Option<NodeId>) -> R<()> {
        // Only the RFC 8040 `rc:yang-data` extension is handled (matches
        // yang-utils.rb): must contain exactly one container, promoted to
        // a top-level root node named "module:<container's own name>" --
        // NOT the yang-data statement's own argument, which is merely a
        // label (e.g. `rc:yang-data coreconf-error { container error {...`
        // promotes to "ietf-coreconf:error", matching the .sid file, not
        // "ietf-coreconf:coreconf-error"). Matches yang-utils.rb:255-256
        // (`container_stmt.arg.prepend(self.name, ':')`).
        if s.keyword == "yang-data" {
            if let Some(container) = s.sub("container") {
                let name = format!("{naming}:{}", container.arg_str());
                let root = self.new_node("container", name.clone(), naming.to_string(), None, true);
                self.interpret_children(naming, naming, &container.subs, Some(root))?;
                // `rc:yang-data` roots aren't children of their module's
                // node (they're "not part of the datastore" -- see
                // finish()'s comment -- so must stay out of module_root's
                // children, which finish() flattens into the real
                // datastore tree) but their own `.sid` file entries (e.g.
                // "/ietf-coreconf:error") still need to resolve to them,
                // since real error responses are addressed by exactly
                // these SIDs on the wire. Register separately so
                // resolve_sid_data_identifier can still find them.
                self.yang_data_roots.insert(name, root);
            }
        }
        Ok(())
    }

    fn qualify_top_level(&self, module: &str, local: &str) -> String {
        format!("{module}:{local}")
    }

    fn interpret_schema_node(
        &mut self,
        resolve: &str,
        naming: &str,
        s: &Stmt,
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

        let node = self.new_node(&s.keyword, name, naming.to_string(), parent, false);

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

        if let Some(st) = s.sub("status") {
            self.nodes[node].status = match st.arg_str() {
                "current" => Some(Status::Current),
                "deprecated" => Some(Status::Deprecated),
                "obsolete" => Some(Status::Obsolete),
                other => return Err(err(format!("unknown status {other:?}"))),
            }
        } else if s.keyword != "input" && s.keyword != "output" {
            self.nodes[node].status = Some(Status::Current);
        }

        if let Some(default) = s.sub("default") {
            self.nodes[node].default = Some(default.arg_str().to_string());
        }

        match s.keyword.as_str() {
            "leaf" | "leaf-list" => {
                if let Some(t) = s.sub("type") {
                    let ty = self.interpret_type(resolve, t)?;
                    self.nodes[node].ty = Some(ty);
                }
            }
            "list" => {
                if let Some(k) = s.sub("key") {
                    self.nodes[node].keys = k
                        .arg_str()
                        .split_whitespace()
                        .map(|s| s.to_string())
                        .collect();
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
                    );
                }
                if s.sub("output").is_none() {
                    self.new_node(
                        "output",
                        "output".to_string(),
                        naming.to_string(),
                        Some(node),
                        false,
                    );
                }
            }
            _ => {}
        }

        if s.keyword == "choice" {
            self.interpret_choice_body(resolve, naming, &s.subs, node)?;
        } else {
            self.interpret_children(resolve, naming, &s.subs, Some(node))?;
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
        stmts: &[Stmt],
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
                );
                self.interpret_one(resolve, naming, s, Some(case_id))?;
                // Implicit case statements should have the same "status" as the data node it represents.
                let child_id = self.nodes[case_id]
                    .children
                    .first()
                    .expect("implicit case must have a child");
                self.nodes[case_id].status = self.nodes[*child_id].status;
            } else {
                self.interpret_one(resolve, naming, s, Some(choice_node))?;
            }
        }
        Ok(())
    }

    fn interpret_uses(
        &mut self,
        resolve: &str,
        naming: &str,
        s: &Stmt,
        parent: Option<NodeId>,
    ) -> R<()> {
        let (def_module, local) = self.split_qualified(resolve, s.arg_str())?;
        let raw = *self
            .defs
            .groupings
            .get(&(def_module.clone(), local.to_string()))
            .ok_or_else(|| {
                err(format!(
                    "grouping {:?} not found (used from {resolve})",
                    s.arg_str()
                ))
            })?;

        // Splice the grouping's body directly into the use site: its own
        // source text resolves against *its* defining module
        // (`def_module` becomes the new `resolve`), but the resulting
        // nodes still belong to the same using-module namespace as
        // before (`naming` passes through unchanged) -- see
        // `interpret_children`'s doc comment.
        self.interpret_children(&def_module, naming, &raw.subs, parent)?;

        // An augment nested inside this `uses` targets a path within the
        // grouping's own body, which was just spliced in above. It's
        // lexically part of the *using* module's text (RFC 7950 7.13
        // treats the whole `uses` body as if copied there), so it
        // resolves and is named in `resolve`/`naming` unchanged, not
        // `def_module`.
        for aug in s.subs_of("augment") {
            self.interpret_augment(resolve, naming, aug, parent, parent)?;
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
        s: &Stmt,
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
            self.interpret_one(resolve, naming, child_stmt, Some(target))?;
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
    /// through.
    fn resolve_sid_data_identifier(&self, path: &str) -> R<NodeId> {
        resolve_schema_path(&self.nodes, &self.module_root, &self.yang_data_roots, path).ok_or_else(
            || {
                err(format!(
                    "target segment not found in SID data identifier {path:?}"
                ))
            },
        )
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

    // -- types ---------------------------------------------------------

    fn resolve_typedef(&mut self, def_module: &str, local: &str) -> R<TypedefId> {
        let key = (def_module.to_string(), local.to_string());
        if let Some(&id) = self.typedef_cache.get(&key) {
            return Ok(id);
        }
        let raw = *self.defs.typedefs.get(&key).ok_or_else(|| {
            err(format!(
                "typedef {local:?} not found in module {def_module:?}"
            ))
        })?;
        let base = raw
            .sub("type")
            .ok_or_else(|| err(format!("typedef {local:?} has no type")))?;
        let ty = self.interpret_type(def_module, base)?;
        let description = raw.sub("description").map(|d| d.arg_str().to_string());
        let id = self.typedefs.len();
        self.typedefs.push(Typedef {
            name: local.to_string(),
            module: def_module.to_string(),
            description,
            ty,
        });
        self.typedef_cache.insert(key, id);
        Ok(id)
    }

    fn interpret_type(&mut self, module: &str, t: &Stmt) -> R<TypeDescriptor> {
        let name = t.arg_str();
        let mut ty = if let Some(builtin) = Builtin::from_name(name) {
            self.interpret_builtin_type(module, builtin, t)?
        } else {
            // typedef reference: clone the named typedef's own
            // descriptor as the starting point for this use site's own
            // restrictions below -- a clone, not a shared reference,
            // since this same typedef can be used (and independently
            // further restricted) at more than one site. Mirrors
            // `inner.derive(typedef['name'])` (yang-utils.rb:324).
            let (def_module, local) = self.split_qualified(module, name)?;
            let typedef_id = self.resolve_typedef(&def_module, local)?;
            let mut ty = self.typedefs[typedef_id].ty.clone();
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
                    let member = self.interpret_type(module, m)?;
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
                                let ty = self.interpret_type(module, t)?;
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
            .chain(self.yang_data_roots.values().copied())
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
                    // A miss here is expected and correct, not a bug: the
                    // generic RFC modules' own .sid files list SIDs for
                    // every node the *standard* defines, but a real
                    // device's own `*-dev.yang` module deviates a good
                    // number of them away (`deviate not-supported`) for
                    // this product profile, and a handful of SIDs belong
                    // to yang-data-extension meta-schema (e.g.
                    // `ietf-sid-file`, describing the .sid file format
                    // itself) that's never addressed on the wire at all.
                    if let Ok(target) = self.resolve_sid_data_identifier(&item.identifier) {
                        self.nodes[target].sid = Some(item.sid);
                    }
                }
                Namespace::Feature => {}
            }
        }
        Ok(())
    }

    fn finish(self) -> R<Schema> {
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
        let yang_data_roots: Vec<NodeId> = self.yang_data_roots.values().copied().collect();
        for yd_root in yang_data_roots {
            self.index_sids(yd_root, &mut sid_index);
        }

        Ok(Schema {
            nodes: self.nodes,
            typedefs: self.typedefs,
            identities: self.identities,
            module_root: self.module_root,
            yang_data_roots: self.yang_data_roots,
            sid_index,
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_defaults_to_current_when_unstated() {
        let src = r#"
            module foo {
              namespace "urn:foo";
              prefix f;

              container bar {
                leaf baz {
                  type string;
                }
              }
            }
        "#;
        let schema = build(&[src.to_string()], &[]).unwrap();
        let bar = schema.node_by_data_path("/foo:bar").unwrap();
        assert_eq!(bar.status, Some(Status::Current));
        let baz = bar.find_child("baz").unwrap();
        assert_eq!(baz.status, Some(Status::Current));
    }

    #[test]
    fn status_is_read_from_an_explicit_statement() {
        let src = r#"
            module foo {
              namespace "urn:foo";
              prefix f;

              container bar {
                status deprecated;
                leaf baz {
                  status obsolete;
                  type string;
                }
              }
            }
        "#;
        let schema = build(&[src.to_string()], &[]).unwrap();
        let bar = schema.node_by_data_path("/foo:bar").unwrap();
        assert_eq!(bar.status, Some(Status::Deprecated));
        let baz = bar.find_child("baz").unwrap();
        assert_eq!(baz.status, Some(Status::Obsolete));
    }

    #[test]
    fn an_unrecognized_status_value_is_a_schema_error() {
        let src = r#"
            module foo {
              namespace "urn:foo";
              prefix f;

              leaf bar {
                status future-proofed;
                type string;
              }
            }
        "#;
        let err = build(&[src.to_string()], &[]).err().unwrap();
        assert!(err.0.contains("future-proofed"), "{}", err.0);
    }

    #[test]
    fn an_implicit_case_copies_its_wrapped_childs_status() {
        // `leaf sub` has no explicit `case` wrapper (RFC 7950 7.9.2), so
        // interpret_choice_body synthesizes one -- it must copy `sub`'s
        // own status, not default to Current on its own.
        let src = r#"
            module foo {
              namespace "urn:foo";
              prefix f;

              container bar {
                choice pick {
                  leaf sub {
                    status deprecated;
                    type string;
                  }
                }
              }
            }
        "#;
        let schema = build(&[src.to_string()], &[]).unwrap();
        // choice/case are spliced out of the flattened data-node-path
        // tree, so resolve via schema-node-path to see the synthesized
        // case node.
        let pick = schema.node_by_schema_path("/foo:bar/pick").unwrap();
        assert_eq!(pick.kw, "choice");
        let implicit_case = schema.node_by_schema_path("/foo:bar/pick/sub").unwrap();
        assert_eq!(implicit_case.kw, "case");
        assert_eq!(implicit_case.status, Some(Status::Deprecated));
        let sub = implicit_case.find_child("sub").unwrap();
        assert_eq!(sub.status, Some(Status::Deprecated));
    }

    #[test]
    fn input_and_output_have_no_status_at_all() {
        let src = r#"
            module foo {
              namespace "urn:foo";
              prefix f;

              rpc bar {
                input {
                  leaf x {
                    type string;
                  }
                }
                output {
                  leaf y {
                    type string;
                  }
                }
              }
            }
        "#;
        let schema = build(&[src.to_string()], &[]).unwrap();
        let input = schema.node_by_schema_path("/foo:bar/input").unwrap();
        let output = schema.node_by_schema_path("/foo:bar/output").unwrap();
        assert_eq!(input.status, None);
        assert_eq!(output.status, None);
    }

    #[test]
    fn top_level_uses_with_augment_reaches_module_root_nodes() {
        let src = r#"
            module foo {
              namespace "urn:foo";
              prefix f;

              grouping g {
                container c {
                  leaf existing {
                    type string;
                  }
                }
              }

              uses g {
                augment "c" {
                  leaf x {
                    type string;
                  }
                }
              }
            }
        "#;
        let schema = build(&[src.to_string()], &[]).unwrap();
        assert!(schema.node_by_data_path("/foo:c/x").is_some());
    }

    #[test]
    fn nested_uses_with_augment_resolves_relative_to_the_use_site() {
        let src = r#"
            module foo {
              namespace "urn:foo";
              prefix f;

              grouping g {
                container c {
                  leaf existing {
                    type string;
                  }
                }
              }

              container outer {
                container decoy {
                  container c {
                    leaf wrong-one {
                      type string;
                    }
                  }
                }
                uses g {
                  augment "c" {
                    leaf x {
                      type string;
                    }
                  }
                }
              }
            }
        "#;
        let schema = build(&[src.to_string()], &[]).unwrap();
        let outer = schema.node_by_data_path("/foo:outer").unwrap();
        let c = outer.find_child("c").unwrap();
        let x = c.find_child("x");
        assert!(x.is_some());

        let decoy = outer.find_child("decoy").unwrap();
        let decoy_c = decoy.find_child("c").unwrap();
        let decoy_x = decoy_c.find_child("x");
        assert!(decoy_x.is_none());
    }

    #[test]
    fn identity_by_name_finds_a_declared_identity() {
        let src = r#"
            module foo {
              namespace "urn:foo";
              prefix f;

              identity animal;
              identity dog {
                base animal;
              }
            }
        "#;
        let schema = build(&[src.to_string()], &[]).unwrap();
        let animal = schema.identity_by_name("foo", "animal").unwrap();
        assert_eq!(animal.name, "animal");
        let dog = schema.identity_by_name("foo", "dog").unwrap();
        assert_eq!(dog.base().count(), 1);
        assert_eq!(dog.base().next().unwrap().name, "animal");
        assert!(schema.identity_by_name("foo", "no-such-identity").is_none());
    }

    #[test]
    fn module_by_sid_finds_a_module_with_an_attached_sid() {
        let src = r#"
            module foo {
              namespace "urn:foo";
              prefix f;
            }
        "#;
        let sid = r#"{
            "module-name": "foo",
            "module-revision": "2026-01-01",
            "items": [
                {"namespace": "module", "identifier": "foo", "sid": 5000}
            ]
        }"#;
        let schema = build(&[src.to_string()], &[sid.to_string()]).unwrap();
        let module = schema.module_by_sid(5000).unwrap();
        assert_eq!(module.name, "foo");
        assert!(schema.module_by_sid(9999).is_none());
    }

    #[test]
    fn module_root_nodes_lists_its_own_top_level_nodes_in_order() {
        let src = r#"
            module foo {
              namespace "urn:foo";
              prefix f;

              leaf a {
                type string;
              }
              leaf b {
                type string;
              }
            }
        "#;
        let schema = build(&[src.to_string()], &[]).unwrap();
        let module = schema.module_by_name("foo").unwrap();
        let names: Vec<String> = module
            .root_nodes()
            .map(|n| n.local_name().to_string())
            .collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn typedef_by_name_finds_the_canonical_unrestricted_resolution() {
        // Two leaves reference the same typedef, one narrowing it further
        // with its own `range` -- typedef_by_name must still return the
        // typedef's own declared (unrestricted) type, not either use
        // site's clone.
        let src = r#"
            module foo {
              namespace "urn:foo";
              prefix f;

              typedef percent {
                type uint8 {
                  range "0..100";
                }
              }

              container bar {
                leaf wide {
                  type percent;
                }
                leaf narrow {
                  type percent {
                    range "0..50";
                  }
                }
              }
            }
        "#;
        let schema = build(&[src.to_string()], &[]).unwrap();
        let percent = schema.typedef_by_name("foo", "percent").unwrap();
        assert_eq!(percent.ty().ranges, vec![(0, 100)]);

        let bar = schema.node_by_data_path("/foo:bar").unwrap();
        let narrow = bar.find_child("narrow").unwrap();
        let narrow_ty = narrow.ty().unwrap();
        assert_eq!(narrow_ty.ranges, vec![(0, 50)]);
        assert_eq!(narrow_ty.base().unwrap().name, "percent");

        // The narrowed use site must not have clobbered the canonical
        // entry `typedef_by_name` returns.
        let percent_again = schema.typedef_by_name("foo", "percent").unwrap();
        assert_eq!(percent_again.ty().ranges, vec![(0, 100)]);
    }

    #[test]
    fn typedef_by_name_is_none_for_an_inline_type_or_unknown_name() {
        let src = r#"
            module foo {
              namespace "urn:foo";
              prefix f;

              leaf bar {
                type string;
              }
            }
        "#;
        let schema = build(&[src.to_string()], &[]).unwrap();
        assert!(schema.typedef_by_name("foo", "string").is_none());
        assert!(schema.typedef_by_name("foo", "no-such-typedef").is_none());
    }

    #[test]
    fn an_inline_type_has_no_base_but_a_typedef_use_site_does() {
        // A leaf's own anonymous `type` has no typedef identity to point
        // back to at all; one referencing (and narrowing) a typedef
        // carries `base` as pure provenance -- not the typedef's own
        // entry, which stays unrestricted (see
        // typedef_by_name_finds_the_canonical_unrestricted_resolution).
        let src = r#"
            module foo {
              namespace "urn:foo";
              prefix f;

              typedef percent {
                type uint8 {
                  range "0..100";
                }
              }

              container bar {
                leaf inline {
                  type uint8;
                }
                leaf named {
                  type percent;
                }
              }
            }
        "#;
        let schema = build(&[src.to_string()], &[]).unwrap();
        let bar = schema.node_by_data_path("/foo:bar").unwrap();

        let inline = bar.find_child("inline").unwrap().ty().unwrap();
        assert!(inline.base().is_none());

        let named = bar.find_child("named").unwrap().ty().unwrap();
        let base = named.base().unwrap();
        assert_eq!(base.name, "percent");
        assert_eq!(base.module, "foo");
    }

    #[test]
    fn typedefs_lists_every_named_typedef_exactly_once_with_its_description() {
        let src = r#"
            module foo {
              namespace "urn:foo";
              prefix f;

              typedef percent {
                description "a percentage, 0 to 100";
                type uint8 {
                  range "0..100";
                }
              }

              container bar {
                leaf a {
                  type percent;
                }
                leaf b {
                  type percent {
                    range "0..50";
                  }
                }
              }
            }
        "#;
        let schema = build(&[src.to_string()], &[]).unwrap();
        let names: Vec<String> = schema.typedefs().map(|t| t.name.clone()).collect();
        assert_eq!(names, vec!["percent"]);
        let percent = schema.typedef_by_name("foo", "percent").unwrap();
        assert_eq!(
            percent.description.as_deref(),
            Some("a percentage, 0 to 100")
        );
    }

    #[test]
    fn leafref_resolves_through_a_typedef_to_its_concrete_target() {
        let src = r#"
            module foo {
              namespace "urn:foo";
              prefix f;

              typedef bar-ref {
                type leafref {
                  path "/f:bar/f:name";
                }
              }

              list bar {
                key "name";
                leaf name {
                  type string;
                }
              }

              leaf pick {
                type bar-ref;
              }
            }
        "#;
        let schema = build(&[src.to_string()], &[]).unwrap();
        let pick = schema.node_by_data_path("/foo:pick").unwrap();
        let target = pick.ty().unwrap().leafref_target().unwrap();
        assert_eq!(target.kw, "leaf");
        assert_eq!(target.local_name(), "name");
    }

    #[test]
    fn an_unresolvable_leafref_path_is_a_schema_error() {
        let src = r#"
            module foo {
              namespace "urn:foo";
              prefix f;

              leaf pick {
                type leafref {
                  path "/f:no-such-node";
                }
              }
            }
        "#;
        let err = build(&[src.to_string()], &[]).err().unwrap();
        assert!(err.0.contains("no-such-node"), "{}", err.0);
    }

    #[test]
    fn a_leafref_path_targeting_a_non_leaf_is_a_schema_error() {
        let src = r#"
            module foo {
              namespace "urn:foo";
              prefix f;

              container bar {
                leaf x {
                  type string;
                }
              }

              leaf pick {
                type leafref {
                  path "/f:bar";
                }
              }
            }
        "#;
        let err = build(&[src.to_string()], &[]).err().unwrap();
        assert!(err.0.contains("container"), "{}", err.0);
    }

    #[test]
    fn a_dangling_leafref_on_a_deviated_away_node_is_not_an_error() {
        // `old` is removed entirely by the deviation before leafref
        // resolution runs; its own leafref path targets another node
        // that's *also* removed. Neither should be visited at all --
        // mirrors a real catalog shape (ietf-interfaces's
        // `higher-layer-if`/`lower-layer-if`, leafrefs into
        // `interfaces-state`, with both sides deviated away together by
        // the same device profile).
        let src = r#"
            module foo {
              namespace "urn:foo";
              prefix f;

              container removed-target {
                leaf name {
                  type string;
                }
              }

              leaf old {
                type leafref {
                  path "/f:removed-target/f:name";
                }
              }

              deviation /f:removed-target {
                deviate not-supported;
              }
              deviation /f:old {
                deviate not-supported;
              }
            }
        "#;
        assert!(build(&[src.to_string()], &[]).is_ok());
    }

    #[test]
    fn node_by_schema_path_sees_choice_case_but_node_by_data_path_does_not() {
        let src = r#"
            module foo {
              namespace "urn:foo";
              prefix f;

              container bar {
                choice pick {
                  case picked {
                    leaf sub {
                      type string;
                    }
                  }
                }
              }
            }
        "#;
        let schema = build(&[src.to_string()], &[]).unwrap();

        let via_schema_path = schema
            .node_by_schema_path("/foo:bar/pick/picked/sub")
            .unwrap();
        assert_eq!(via_schema_path.kw, "leaf");

        assert!(schema.node_by_schema_path("/foo:bar/pick").is_some());
        assert_eq!(
            schema.node_by_schema_path("/foo:bar/pick").unwrap().kw,
            "choice"
        );
        assert!(schema.node_by_schema_path("/foo:bar/pick/picked").is_some());
        assert_eq!(
            schema
                .node_by_schema_path("/foo:bar/pick/picked")
                .unwrap()
                .kw,
            "case"
        );

        // The same leaf is reachable by data-node-path with the choice
        // and case segments simply absent -- and naming either of them
        // finds nothing at all, since both were spliced out of the
        // flattened tree.
        let via_data_path = schema.node_by_data_path("/foo:bar/sub").unwrap();
        assert_eq!(via_data_path.kw, "leaf");
        assert!(schema
            .node_by_data_path("/foo:bar/pick/picked/sub")
            .is_none());
        assert!(schema.node_by_data_path("/foo:bar/pick").is_none());
        assert!(schema.node_by_data_path("/foo:bar/picked").is_none());
    }

    #[test]
    fn node_by_schema_path_disambiguates_the_first_segment_by_module() {
        let src = r#"
            module foo {
              namespace "urn:foo";
              prefix f;

              container shared {
                leaf x {
                  type string;
                }
              }
            }
        "#;
        let src2 = r#"
            module bar {
              namespace "urn:bar";
              prefix b;

              container shared {
                leaf y {
                  type string;
                }
              }
            }
        "#;
        let schema = build(&[src.to_string(), src2.to_string()], &[]).unwrap();

        let foo_shared = schema.node_by_schema_path("/foo:shared").unwrap();
        assert!(foo_shared.find_child("x").is_some());
        assert!(foo_shared.find_child("y").is_none());

        let bar_shared = schema.node_by_schema_path("/bar:shared").unwrap();
        assert!(bar_shared.find_child("y").is_some());
        assert!(bar_shared.find_child("x").is_none());

        assert!(schema.node_by_data_path("/foo:shared/x").is_some());
        assert!(schema.node_by_data_path("/bar:shared/y").is_some());
        assert!(schema.node_by_data_path("/foo:shared/y").is_none());
    }

    #[test]
    fn node_by_schema_path_finds_a_yang_data_root_by_its_full_qualified_name() {
        let src = r#"
            module foo {
              namespace "urn:foo";
              prefix f;

              rc:yang-data coreconf-error {
                container error {
                  leaf message {
                    type string;
                  }
                }
              }
            }
        "#;
        let schema = build(&[src.to_string()], &[]).unwrap();
        let error_root = schema.node_by_schema_path("/foo:error").unwrap();
        assert_eq!(error_root.kw, "container");
        assert!(error_root.find_child("message").is_some());

        // Not part of the datastore, so unreachable via a data-node-path.
        assert!(schema.node_by_data_path("/foo:error").is_none());
    }

    #[test]
    fn node_by_schema_path_and_node_by_data_path_return_none_for_unknown_paths() {
        let src = r#"
            module foo {
              namespace "urn:foo";
              prefix f;

              leaf bar {
                type string;
              }
            }
        "#;
        let schema = build(&[src.to_string()], &[]).unwrap();
        assert!(schema.node_by_schema_path("/foo:no-such-node").is_none());
        assert!(schema.node_by_schema_path("/no-such-module:bar").is_none());
        assert!(schema.node_by_data_path("/foo:no-such-node").is_none());
        assert!(schema.node_by_schema_path("").is_none());
        assert!(schema.node_by_data_path("").is_none());
    }
}
