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

pub type NodeId = usize;
pub type TypeId = usize;
pub type IdentityId = usize;

const SCHEMA_NODE_KEYWORDS: &[&str] = &[
    "container", "leaf", "leaf-list", "list", "choice", "case", "rpc", "action", "input", "output",
    "notification", "anydata", "anyxml",
];

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
pub struct TypeDef {
    pub builtin: Builtin,
    pub fraction_digits: Option<u8>,
    pub bits: Vec<Bit>,
    pub enums: Vec<EnumVal>,
    pub identity_bases: Vec<IdentityId>,
    /// The module that declared this identityref usage -- an unprefixed
    /// identity value name is disambiguated against this module.
    pub source_module: Option<String>,
    pub leafref_path: Option<String>,
    pub leafref_target: Option<NodeId>,
    pub union_members: Vec<TypeId>,
}

impl TypeDef {
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
        }
    }
}

#[derive(Debug, Clone)]
pub struct Identity {
    pub name: String,
    pub module: String,
    pub base: Vec<IdentityId>,
    pub derived: Vec<IdentityId>,
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
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub config: bool,
    pub mandatory: bool,
    pub type_id: Option<TypeId>,
    /// List keys, in schema-declared order (not necessarily input order).
    pub keys: Vec<String>,
    pub sid: Option<i64>,
    pub owner_module: String,
}

impl Node {
    pub fn local_name(&self) -> &str {
        self.name.rsplit_once(':').map(|(_, n)| n).unwrap_or(&self.name)
    }
}

pub struct Schema {
    pub nodes: Vec<Node>,
    pub types: Vec<TypeDef>,
    pub identities: Vec<Identity>,
    pub identity_by_module_name: HashMap<(String, String), IdentityId>,
    /// Each module's own (un-flattened) top-level schema root, keyed by
    /// module name -- used for augment-target and SID resolution.
    pub module_root: HashMap<String, NodeId>,
    /// The synthetic flattened data-tree-schema root (choice/case elided),
    /// used by the codec. `sid == Some(0)`.
    pub root: NodeId,
    /// Absolute SID -> node in the flattened tree, for FETCH/decode.
    pub sid_index: HashMap<i64, NodeId>,
}

impl Schema {
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }
    pub fn ty(&self, id: TypeId) -> &TypeDef {
        &self.types[id]
    }
    pub fn identity(&self, id: IdentityId) -> &Identity {
        &self.identities[id]
    }

    pub fn find_child(&self, parent: NodeId, name: &str) -> Option<NodeId> {
        self.nodes[parent].children.iter().copied().find(|&c| self.nodes[c].name == name)
    }

    /// All identities transitively derived from `base` (including `base`
    /// itself), mirroring `Identity#derived_from`.
    pub fn derived_from(&self, base: IdentityId) -> std::collections::HashSet<IdentityId> {
        let mut out = std::collections::HashSet::new();
        let mut stack = vec![base];
        while let Some(id) = stack.pop() {
            if out.insert(id) {
                stack.extend(self.identities[id].derived.iter().copied());
            }
        }
        out
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

pub struct Builder<'a> {
    raw_modules: HashMap<String, &'a Stmt>,
    contexts: HashMap<String, ModuleCtx>,
    defs: Definitions<'a>,
    building: std::collections::HashSet<String>,
    built: std::collections::HashSet<String>,

    nodes: Vec<Node>,
    types: Vec<TypeDef>,
    identities: Vec<Identity>,
    identity_by_module_name: HashMap<(String, String), IdentityId>,
    module_root: HashMap<String, NodeId>,
    yang_data_roots: HashMap<String, NodeId>,
    leafref_pending: Vec<TypeId>,
}

/// Parse `.yang` sources and build the full schema, including flattening
/// and SID attachment from `.sid` sources. Fresh every call -- no caching,
/// by design (see `mup1cc-rs/rust-mup1cc.txt`).
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
        defs: Definitions { typedefs, groupings },
        building: Default::default(),
        built: Default::default(),
        nodes: Vec::new(),
        types: Vec::new(),
        identities: Vec::new(),
        identity_by_module_name: HashMap::new(),
        module_root: HashMap::new(),
        yang_data_roots: HashMap::new(),
        leafref_pending: Vec::new(),
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
    fn new_node(&mut self, kw: &str, name: String, owner_module: String, parent: Option<NodeId>) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(Node {
            kw: kw.to_string(),
            name,
            parent,
            children: Vec::new(),
            config: true,
            mandatory: false,
            type_id: None,
            keys: Vec::new(),
            sid: None,
            owner_module,
        });
        if let Some(p) = parent {
            self.nodes[p].children.push(id);
        }
        id
    }

    fn get_identity(&mut self, module: &str, name: &str) -> IdentityId {
        let key = (module.to_string(), name.to_string());
        if let Some(&id) = self.identity_by_module_name.get(&key) {
            return id;
        }
        let id = self.identities.len();
        self.identities.push(Identity { name: name.to_string(), module: module.to_string(), base: Vec::new(), derived: Vec::new(), sid: None });
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

        let raw = *self.raw_modules.get(name).ok_or_else(|| err(format!("unknown module {name:?}")))?;
        let imports: Vec<String> = raw.subs_of("import").map(|i| i.arg_str().to_string()).collect();
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

        let root = self.new_node("module", name.to_string(), name.to_string(), None);
        self.module_root.insert(name.to_string(), root);

        let ctx_name = name.to_string();
        self.interpret_children(&ctx_name, &ctx_name, &raw.subs, root)?;

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
                let ctx = self.contexts.get(current_module).ok_or_else(|| err(format!("unknown module {current_module:?}")))?;
                let module = ctx
                    .resolve_prefix(prefix)
                    .ok_or_else(|| err(format!("unknown prefix {prefix:?} in module {current_module:?}")))?
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
    fn interpret_children(&mut self, resolve: &str, naming: &str, stmts: &[Stmt], parent: NodeId) -> R<()> {
        for s in stmts {
            self.interpret_one(resolve, naming, s, parent)?;
        }
        Ok(())
    }

    fn interpret_one(&mut self, resolve: &str, naming: &str, s: &Stmt, parent: NodeId) -> R<()> {
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

    fn interpret_extension(&mut self, naming: &str, s: &Stmt, _parent: NodeId) -> R<()> {
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
                let root = self.new_node("container", name.clone(), naming.to_string(), None);
                self.interpret_children(naming, naming, &container.subs, root)?;
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

    fn interpret_schema_node(&mut self, resolve: &str, naming: &str, s: &Stmt, parent: NodeId) -> R<()> {
        let is_top = self.nodes[parent].kw == "module";
        // input/output are the only schema-node statements RFC 7950 gives
        // no argument to (there's always at most one of each per
        // rpc/action) -- use the keyword itself as the name.
        let local = match s.keyword.as_str() {
            "input" | "output" => s.keyword.as_str(),
            _ => s.arg_str(),
        };
        let name = if is_top { self.qualify_top_level(naming, local) } else { local.to_string() };

        let node = self.new_node(&s.keyword, name, naming.to_string(), Some(parent));

        if let Some(cfg) = s.sub("config") {
            self.nodes[node].config = cfg.arg_str() == "true";
        } else if let Some(p) = self.nodes[node].parent {
            self.nodes[node].config = self.nodes[p].config;
        }
        if let Some(m) = s.sub("mandatory") {
            self.nodes[node].mandatory = m.arg_str() == "true";
        }

        match s.keyword.as_str() {
            "leaf" | "leaf-list" => {
                if let Some(t) = s.sub("type") {
                    let ty = self.interpret_type(resolve, t)?;
                    self.nodes[node].type_id = Some(ty);
                }
            }
            "list" => {
                if let Some(k) = s.sub("key") {
                    self.nodes[node].keys = k.arg_str().split_whitespace().map(|s| s.to_string()).collect();
                }
            }
            "rpc" | "action" => {
                if s.sub("input").is_none() {
                    self.new_node("input", "input".to_string(), naming.to_string(), Some(node));
                }
                if s.sub("output").is_none() {
                    self.new_node("output", "output".to_string(), naming.to_string(), Some(node));
                }
            }
            _ => {}
        }

        if s.keyword == "choice" {
            self.interpret_choice_body(resolve, naming, &s.subs, node)?;
        } else {
            self.interpret_children(resolve, naming, &s.subs, node)?;
        }
        Ok(())
    }

    /// A choice's direct container/leaf/leaf-list/list/choice/anydata/
    /// anyxml child is wrapped in a synthetic, unnamed case sharing the
    /// child's own name (RFC 7950 7.9.2); an explicit `case` is used as
    /// declared.
    fn interpret_choice_body(&mut self, resolve: &str, naming: &str, stmts: &[Stmt], choice_node: NodeId) -> R<()> {
        const IMPLICIT_CASE_KEYWORDS: &[&str] =
            &["container", "leaf", "leaf-list", "list", "choice", "anydata", "anyxml"];
        for s in stmts {
            if s.prefix.is_none() && IMPLICIT_CASE_KEYWORDS.contains(&s.keyword.as_str()) {
                let case_id = self.new_node("case", s.arg_str().to_string(), naming.to_string(), Some(choice_node));
                self.interpret_one(resolve, naming, s, case_id)?;
            } else {
                self.interpret_one(resolve, naming, s, choice_node)?;
            }
        }
        Ok(())
    }

    fn interpret_uses(&mut self, resolve: &str, naming: &str, s: &Stmt, parent: NodeId) -> R<()> {
        let (def_module, local) = self.split_qualified(resolve, s.arg_str())?;
        let raw = *self
            .defs
            .groupings
            .get(&(def_module.clone(), local.to_string()))
            .ok_or_else(|| err(format!("grouping {:?} not found (used from {resolve})", s.arg_str())))?;

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
            self.interpret_augment(resolve, naming, aug, parent, Some(parent))?;
        }
        Ok(())
    }

    /// `relative_root`: `Some(parent)` for an augment nested in `uses`
    /// (relative to the use site); `None` for a module-level augment
    /// (absolute path from a module root). `resolve` locates the target
    /// (its own import-prefix table, for the absolute-path case);
    /// `naming` is compared against the target's owner to decide whether
    /// newly-added children need cross-module qualification.
    fn interpret_augment(&mut self, resolve: &str, naming: &str, s: &Stmt, _use_site: NodeId, relative_root: Option<NodeId>) -> R<()> {
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
            self.interpret_one(resolve, naming, child_stmt, target)?;
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
            cur = self.find_by_local(cur, local).ok_or_else(|| err(format!("augment target segment {seg:?} not found (relative path {path:?})")))?;
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
        let mut segs = path.split('/').filter(|s| !s.is_empty());
        let first = segs.next().ok_or_else(|| err(format!("empty SID data identifier {path:?}")))?;
        // An `rc:yang-data` root (e.g. "/ietf-coreconf:error") is matched
        // by its full qualified name directly -- it has no module-root
        // parent to descend from (see interpret_extension).
        let mut cur = if let Some(&root) = self.yang_data_roots.get(first) {
            root
        } else {
            let (dst_module, first_local) = first
                .split_once(':')
                .ok_or_else(|| err(format!("first segment {first:?} of SID data identifier {path:?} has no module prefix")))?;
            let dst_root = *self
                .module_root
                .get(dst_module)
                .ok_or_else(|| err(format!("unknown module {dst_module:?} in SID data identifier {path:?}")))?;
            self.find_by_local(dst_root, first_local)
                .ok_or_else(|| err(format!("target segment {first_local:?} not found in module {dst_module:?} ({path:?})")))?
        };
        for seg in segs {
            let local = seg.rsplit_once(':').map(|(_, n)| n).unwrap_or(seg);
            cur = self.find_by_local(cur, local).ok_or_else(|| err(format!("target segment {seg:?} not found ({path:?})")))?;
        }
        Ok(cur)
    }

    fn find_by_local(&self, parent: NodeId, local: &str) -> Option<NodeId> {
        self.nodes[parent].children.iter().copied().find(|&c| self.nodes[c].local_name() == local)
    }

    /// Resolve an absolute schema-node-id (`/prefix:top/child/...`) using
    /// `current_module`'s own import-prefix map for the first segment.
    fn resolve_absolute_schema_node_id(&self, current_module: &str, path: &str) -> R<NodeId> {
        let mut segs = path.split('/').filter(|s| !s.is_empty());
        let first = segs.next().ok_or_else(|| err(format!("empty schema-node-id {path:?}")))?;
        let (dst_module, first_local) = self.split_qualified(current_module, first)?;
        let dst_root = *self.module_root.get(&dst_module).ok_or_else(|| err(format!("unknown target module {dst_module:?} in {path:?}")))?;
        let mut cur = self.find_by_local(dst_root, first_local).ok_or_else(|| err(format!("target segment {first_local:?} not found in module {dst_module:?} ({path:?})")))?;
        for seg in segs {
            let local = seg.rsplit_once(':').map(|(_, n)| n).unwrap_or(seg);
            cur = self.find_by_local(cur, local).ok_or_else(|| err(format!("target segment {seg:?} not found ({path:?})")))?;
        }
        Ok(cur)
    }

    // -- types ---------------------------------------------------------

    fn new_type(&mut self, t: TypeDef) -> TypeId {
        let id = self.types.len();
        self.types.push(t);
        id
    }

    fn interpret_type(&mut self, module: &str, t: &Stmt) -> R<TypeId> {
        let name = t.arg_str();
        if let Some(builtin) = Builtin::from_name(name) {
            return self.interpret_builtin_type(module, builtin, t);
        }
        // typedef: interpret its base type, carrying restrictions forward
        // as a fresh TypeDef (a full "derive" isn't needed since we don't
        // track range/length/pattern -- see module docs).
        let (def_module, local) = self.split_qualified(module, name)?;
        let raw = *self
            .defs
            .typedefs
            .get(&(def_module.clone(), local.to_string()))
            .ok_or_else(|| err(format!("typedef {name:?} not found (used from {module})")))?;
        let base = raw.sub("type").ok_or_else(|| err(format!("typedef {name:?} has no type")))?;
        self.interpret_type(&def_module, base)
    }

    fn interpret_builtin_type(&mut self, module: &str, builtin: Builtin, t: &Stmt) -> R<TypeId> {
        let mut ty = TypeDef::new(builtin);

        match builtin {
            Builtin::Decimal64 => {
                if let Some(fd) = t.sub("fraction-digits") {
                    ty.fraction_digits = fd.arg_str().parse().ok();
                }
            }
            Builtin::Bits => {
                let mut next_pos = 0u32;
                for b in t.subs_of("bit") {
                    let position = b.sub("position").and_then(|p| p.arg_str().parse().ok()).unwrap_or(next_pos);
                    next_pos = position + 1;
                    ty.bits.push(Bit { name: b.arg_str().to_string(), position });
                }
            }
            Builtin::Enumeration => {
                let mut next_val = 0i64;
                for e in t.subs_of("enum") {
                    let value = e.sub("value").and_then(|v| v.arg_str().parse().ok()).unwrap_or(next_val);
                    next_val = value + 1;
                    ty.enums.push(EnumVal { name: e.arg_str().to_string(), value });
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

        Ok(self.new_type(ty))
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
                                self.nodes[target].type_id = Some(ty);
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
        if let Some(p) = self.nodes[target].parent {
            self.nodes[p].children.retain(|&c| c != target);
        }
    }

    fn propagate_config_false(&mut self, node: NodeId) {
        let children = self.nodes[node].children.clone();
        for c in children {
            self.nodes[c].config = false;
            self.propagate_config_false(c);
        }
    }

    // -- leafref ---------------------------------------------------------

    fn resolve_leafrefs(&mut self) -> R<()> {
        // Map each Leafref TypeId to the node that declared it -- needed
        // to resolve a *relative* path ("../../module/identifier"),
        // which starts from that leaf's own position in the tree, not
        // from any fixed module root.
        let mut owner: HashMap<TypeId, NodeId> = HashMap::new();
        for (id, node) in self.nodes.iter().enumerate() {
            if let Some(t) = node.type_id {
                owner.insert(t, id);
            }
        }

        for i in 0..self.types.len() {
            if self.types[i].builtin == Builtin::Leafref && self.types[i].leafref_path.is_some() {
                self.leafref_pending.push(i);
            }
        }
        for ty_id in self.leafref_pending.clone() {
            let path = self.types[ty_id].leafref_path.clone().unwrap();
            let module = self.types[ty_id].source_module.clone().unwrap();
            let target = if let Some(abs) = path.trim().strip_prefix('/') {
                self.resolve_absolute_schema_node_id(&module, &format!("/{abs}")).ok()
            } else {
                owner.get(&ty_id).and_then(|&node| self.resolve_relative_leafref(node, &path))
            };
            if let Some(target) = target {
                self.types[ty_id].leafref_target = Some(target);
            }
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
                    if let Some(&root) = self.module_root.get(&item.identifier) {
                        self.nodes[root].sid = Some(item.sid);
                    }
                }
                Namespace::Identity => {
                    if let Some(&id) = self.identity_by_module_name.get(&(f.module_name.clone(), item.identifier.clone())) {
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

    // -- finalize: flatten choice/case into the data-tree schema ---------

    fn finish(mut self) -> R<Schema> {
        let root = self.new_node("module", "data-tree-schema".to_string(), String::new(), None);
        self.nodes[root].sid = Some(0);

        let module_roots: Vec<NodeId> = self.module_root.values().copied().collect();
        for m_root in module_roots {
            let top_children = self.nodes[m_root].children.clone();
            for child in top_children {
                // flatten_clone already appends the new node to `root`'s
                // children itself (needed so a spliced choice/case can
                // contribute more than one).
                self.flatten_clone(child, Some(root));
            }
        }

        let mut sid_index = HashMap::new();
        self.index_sids(root, &mut sid_index);
        // `rc:yang-data` roots (e.g. `ietf-coreconf:error`) are
        // deliberately not children of `root` -- a whole-tree GET/PUT
        // must not enumerate them as if they were real datastore paths --
        // but their SIDs are still real wire values (a device error
        // response is addressed by exactly these SIDs), so index each
        // one's subtree separately rather than via the `root` walk.
        let yang_data_roots: Vec<NodeId> = self.yang_data_roots.values().copied().collect();
        for yd_root in yang_data_roots {
            self.index_sids(yd_root, &mut sid_index);
        }

        Ok(Schema {
            nodes: self.nodes,
            types: self.types,
            identities: self.identities,
            identity_by_module_name: self.identity_by_module_name,
            module_root: self.module_root,
            root,
            sid_index,
        })
    }

    /// Clone `node` (and, recursively, its children) into a fresh node
    /// under `new_parent`, splicing away `choice`/`case` wrappers by
    /// re-parenting their children directly (RFC 7951/9254: choice/case
    /// are schema-tree-only, never data nodes).
    fn flatten_clone(&mut self, node: NodeId, new_parent: Option<NodeId>) -> NodeId {
        if matches!(self.nodes[node].kw.as_str(), "choice" | "case") {
            // Splice: this node contributes its children directly, no
            // node of its own in the flattened tree. Since exactly one
            // flattened id is expected by the caller, promote the first
            // spliced child and append the rest as extra siblings.
            let children = self.nodes[node].children.clone();
            let mut ids = Vec::new();
            for c in children {
                ids.extend(self.flatten_clone_multi(c, new_parent));
            }
            return *ids.first().unwrap_or(&self.dangling_placeholder(new_parent));
        }

        let src = &self.nodes[node];
        let new_id = self.nodes.len();
        self.nodes.push(Node {
            kw: src.kw.clone(),
            name: src.name.clone(),
            parent: new_parent,
            children: Vec::new(),
            config: src.config,
            mandatory: src.mandatory,
            type_id: src.type_id,
            keys: src.keys.clone(),
            sid: src.sid,
            owner_module: src.owner_module.clone(),
        });
        if let Some(p) = new_parent {
            self.nodes[p].children.push(new_id);
        }
        let children = self.nodes[node].children.clone();
        for c in children {
            self.flatten_clone_multi(c, Some(new_id));
        }
        new_id
    }

    /// Like `flatten_clone`, but returns every id actually added as a
    /// direct child of `new_parent` (more than one when `node` is a
    /// choice/case being spliced away).
    fn flatten_clone_multi(&mut self, node: NodeId, new_parent: Option<NodeId>) -> Vec<NodeId> {
        if matches!(self.nodes[node].kw.as_str(), "choice" | "case") {
            let children = self.nodes[node].children.clone();
            let mut ids = Vec::new();
            for c in children {
                ids.extend(self.flatten_clone_multi(c, new_parent));
            }
            ids
        } else {
            vec![self.flatten_clone(node, new_parent)]
        }
    }

    fn dangling_placeholder(&mut self, parent: Option<NodeId>) -> NodeId {
        // An empty choice/case with no data-node children at all (legal
        // but useless); create nothing and let callers treat this as
        // absent. Only reached for genuinely empty choices.
        self.new_node("case", String::new(), String::new(), parent)
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
