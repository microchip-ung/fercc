// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

use serde_json::{Map, Value as Json};

use crate::json_schema::num;
use crate::schema::{
    default_ranges, Builtin, IdentityRef, ModuleRef, NodeRef, Schema, Status, TypeRef, TypedefRef,
};

pub const SCHEMA_VERSION: &str = "1.0.0";
const SCHEMA_URL: &str = "urn:fercc:resolved-yang-catalog-schema:1.0.0";

type R<T> = Result<T, String>;

fn s(v: impl Into<String>) -> Json {
    Json::String(v.into())
}

fn arr(items: Vec<Json>) -> Json {
    Json::Array(items)
}

fn obj(pairs: Vec<(&str, Json)>) -> Json {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    Json::Object(m)
}

fn opt_insert(m: &mut Map<String, Json>, key: &str, value: Option<Json>) {
    if let Some(v) = value {
        m.insert(key.to_string(), v);
    }
}

fn status_str(status: Status) -> &'static str {
    match status {
        Status::Current => "current",
        Status::Deprecated => "deprecated",
        Status::Obsolete => "obsolete",
    }
}

fn builtin_str(b: Builtin) -> &'static str {
    match b {
        Builtin::Binary => "binary",
        Builtin::Bits => "bits",
        Builtin::Boolean => "boolean",
        Builtin::Decimal64 => "decimal64",
        Builtin::Empty => "empty",
        Builtin::Enumeration => "enumeration",
        Builtin::Identityref => "identityref",
        Builtin::InstanceIdentifier => "instance-identifier",
        Builtin::Int8 => "int8",
        Builtin::Int16 => "int16",
        Builtin::Int32 => "int32",
        Builtin::Int64 => "int64",
        Builtin::Uint8 => "uint8",
        Builtin::Uint16 => "uint16",
        Builtin::Uint32 => "uint32",
        Builtin::Uint64 => "uint64",
        Builtin::Leafref => "leafref",
        Builtin::String => "string",
        Builtin::Union => "union",
    }
}

fn qualified_name(module: &str, name: &str) -> String {
    format!("{module}:{name}")
}

fn typedef_key(scope: &str, name: &str) -> String {
    format!("{scope} {name}")
}

fn sid_string(sid: Option<i64>, what: &str) -> R<String> {
    sid.map(|v| v.to_string())
        .ok_or_else(|| format!("{what} has no sid -- this catalog's .sid files are incomplete"))
}

fn range_bounds(ranges: &[(i128, i128)]) -> Json {
    arr(ranges
        .iter()
        .map(|&(min, max)| arr(vec![num(min), num(max)]))
        .collect())
}

fn range64_bounds(ranges: &[(i128, i128)]) -> Json {
    arr(ranges
        .iter()
        .map(|&(min, max)| arr(vec![s(min.to_string()), s(max.to_string())]))
        .collect())
}

fn range_object(ranges: &[(i128, i128)]) -> Json {
    obj(vec![("bounds", range_bounds(ranges))])
}

fn range64_object(ranges: &[(i128, i128)]) -> Json {
    obj(vec![("bounds", range64_bounds(ranges))])
}

fn is_explicitly_restricted(ty: TypeRef<'_>) -> bool {
    ty.ranges != default_ranges(ty.builtin)
}

fn type_descriptor(ty: TypeRef<'_>) -> R<Json> {
    let mut m = Map::new();
    m.insert("builtin".to_string(), s(builtin_str(ty.builtin)));
    opt_insert(
        &mut m,
        "derived-from",
        ty.base()
            .map(|base| s(typedef_key(&base.scope, &base.name))),
    );

    match ty.builtin {
        Builtin::Binary => {
            if is_explicitly_restricted(ty) {
                m.insert("length".to_string(), range_object(&ty.ranges));
            }
        }
        Builtin::Bits => {
            m.insert(
                "bit".to_string(),
                arr(ty
                    .bits
                    .iter()
                    .map(|b| {
                        obj(vec![
                            ("name", s(b.name.clone())),
                            ("position", Json::Number(b.position.into())),
                        ])
                    })
                    .collect()),
            );
        }
        Builtin::Boolean => {}
        Builtin::Decimal64 => {
            let fd = ty.fraction_digits.ok_or(
                "a decimal64 type is missing its fraction-digits substatement (RFC 7950 9.3.4)",
            )?;
            m.insert("fraction-digit".to_string(), Json::Number(fd.into()));
            if is_explicitly_restricted(ty) {
                m.insert("range".to_string(), range64_object(&ty.ranges));
            }
        }
        Builtin::Empty => {}
        Builtin::Enumeration => {
            m.insert(
                "enum".to_string(),
                arr(ty
                    .enums
                    .iter()
                    .map(|e| {
                        obj(vec![
                            ("name", s(e.name.clone())),
                            ("value", Json::Number(e.value.into())),
                        ])
                    })
                    .collect()),
            );
        }
        Builtin::Identityref => {
            m.insert(
                "base".to_string(),
                arr(ty
                    .identity_bases()
                    .map(|b| s(qualified_name(&b.module, &b.name)))
                    .collect()),
            );
        }
        Builtin::InstanceIdentifier => {}
        Builtin::Int8
        | Builtin::Int16
        | Builtin::Int32
        | Builtin::Uint8
        | Builtin::Uint16
        | Builtin::Uint32 => {
            if is_explicitly_restricted(ty) {
                m.insert("range".to_string(), range_object(&ty.ranges));
            }
        }
        Builtin::Int64 | Builtin::Uint64 => {
            if is_explicitly_restricted(ty) {
                m.insert("range".to_string(), range64_object(&ty.ranges));
            }
        }
        Builtin::Leafref => {
            let path = ty
                .leafref_path
                .as_deref()
                .ok_or("a leafref type is missing its path substatement (RFC 7950 9.9.2)")?;
            m.insert("path".to_string(), s(path.to_string()));
            let target = ty.leafref_target().ok_or_else(|| {
                format!("leafref path {path:?} could not be resolved to a target")
            })?;
            m.insert("target".to_string(), s(target.path()));
        }
        Builtin::String => {
            if is_explicitly_restricted(ty) {
                m.insert("length".to_string(), range_object(&ty.ranges));
            }
            if !ty.patterns.is_empty() {
                m.insert(
                    "pattern".to_string(),
                    arr(ty
                        .patterns
                        .iter()
                        .map(|p| obj(vec![("pattern", s(p.clone()))]))
                        .collect()),
                );
            }
        }
        Builtin::Union => {
            let members: Vec<Json> = ty
                .union_members()
                .map(type_descriptor)
                .collect::<R<Vec<_>>>()?;
            m.insert("type".to_string(), arr(members));
        }
    }

    Ok(Json::Object(m))
}

fn typedef_entry(t: TypedefRef<'_>) -> R<Json> {
    let mut m = Map::new();
    m.insert("module".to_string(), s(t.module.clone()));
    m.insert("name".to_string(), s(t.name.clone()));
    opt_insert(&mut m, "description", t.description.clone().map(s));
    m.insert("type".to_string(), type_descriptor(t.ty())?);
    Ok(Json::Object(m))
}

fn identity_entry(identity: IdentityRef<'_>) -> R<Json> {
    let mut m = Map::new();
    m.insert("module".to_string(), s(identity.module.clone()));
    m.insert("name".to_string(), s(identity.name.clone()));
    opt_insert(&mut m, "description", identity.description.clone().map(s));
    m.insert(
        "sid".to_string(),
        s(sid_string(
            identity.sid,
            &format!("identity {}:{}", identity.module, identity.name),
        )?),
    );
    m.insert(
        "base".to_string(),
        arr(identity
            .base()
            .map(|b| s(qualified_name(&b.module, &b.name)))
            .collect()),
    );
    m.insert(
        "derived".to_string(),
        arr(identity
            .derived()
            .filter(|d| d.sid.is_some())
            .map(|d| s(qualified_name(&d.module, &d.name)))
            .collect()),
    );
    Ok(Json::Object(m))
}

fn schema_node_entry(node: NodeRef<'_>) -> R<Json> {
    let mut m = Map::new();
    m.insert("keyword".to_string(), s(node.kw.clone()));
    m.insert("name".to_string(), s(node.local_name().to_string()));
    m.insert("module".to_string(), s(node.owner_module.clone()));
    m.insert(
        "parent".to_string(),
        node.parent().map(|p| s(p.path())).unwrap_or(Json::Null),
    );
    m.insert(
        "children".to_string(),
        arr(node.children().map(|c| s(c.path())).collect()),
    );
    m.insert(
        "data-children".to_string(),
        arr(node.data_children().map(|c| s(c.path())).collect()),
    );
    if let Some(status) = node.status {
        m.insert("status".to_string(), s(status_str(status)));
    }
    opt_insert(&mut m, "description", node.description.clone().map(s));
    if matches!(node.kw.as_str(), "choice" | "case") {
        opt_insert(&mut m, "sid", node.sid.map(|v| s(v.to_string())));
    } else if node.synthetic && matches!(node.kw.as_str(), "input" | "output") && node.sid.is_none()
    {
        // TODO: known pyang bug (RFC 9595 6.4 requires a sid here) --
        // some catalogs omit the sid on a synthetic input/output node.
        // Remove this branch once every supported catalog's pyang
        // generation is fixed and always assigns one.
        eprintln!(
            "warning: synthetic schema node {} has no sid (known pyang bug) -- omitting it",
            node.path()
        );
    } else {
        m.insert(
            "sid".to_string(),
            s(sid_string(
                node.sid,
                &format!("schema node {}", node.path()),
            )?),
        );
    }
    m.insert("synthetic".to_string(), Json::Bool(node.synthetic));

    match node.kw.as_str() {
        "container" => {
            m.insert("config".to_string(), Json::Bool(node.config));
        }
        "leaf" => {
            m.insert("config".to_string(), Json::Bool(node.config));
            let ty = node
                .ty()
                .ok_or_else(|| format!("leaf {} has no type", node.path()))?;
            m.insert("type".to_string(), type_descriptor(ty)?);
            m.insert("mandatory".to_string(), Json::Bool(node.mandatory));
            opt_insert(&mut m, "default", node.default.clone().map(s));
        }
        "leaf-list" => {
            m.insert("config".to_string(), Json::Bool(node.config));
            let ty = node
                .ty()
                .ok_or_else(|| format!("leaf-list {} has no type", node.path()))?;
            m.insert("type".to_string(), type_descriptor(ty)?);
            opt_insert(
                &mut m,
                "default",
                node.default.clone().map(|d| arr(vec![s(d)])),
            );
        }
        "list" => {
            m.insert("config".to_string(), Json::Bool(node.config));
            m.insert(
                "keys".to_string(),
                arr(node.keys().map(|k| s(k.path())).collect()),
            );
        }
        "choice" => {
            m.insert("config".to_string(), Json::Bool(node.config));
            m.insert("mandatory".to_string(), Json::Bool(node.mandatory));
            opt_insert(&mut m, "default", node.default.clone().map(s));
        }
        "case" | "rpc" | "action" | "input" | "output" | "notification" => {}
        "anydata" | "anyxml" => {
            m.insert("config".to_string(), Json::Bool(node.config));
            m.insert("mandatory".to_string(), Json::Bool(node.mandatory));
        }
        other => return Err(format!("unexpected schema-node keyword {other:?}")),
    }

    Ok(Json::Object(m))
}

fn module_entry(module: ModuleRef<'_>, schema: &Schema) -> R<Json> {
    let mut m = Map::new();
    m.insert("namespace".to_string(), s(module.namespace.clone()));
    m.insert(
        "sid".to_string(),
        s(sid_string(module.sid, &format!("module {}", module.name))?),
    );
    opt_insert(&mut m, "revision", module.revision.clone().map(s));
    m.insert(
        "root-nodes".to_string(),
        arr(module.root_nodes().map(|n| s(n.path())).collect()),
    );
    m.insert(
        "data-root-nodes".to_string(),
        arr(module.root_data_nodes().map(|n| s(n.path())).collect()),
    );
    m.insert(
        "typedefs".to_string(),
        arr(schema
            .typedefs()
            .filter(|t| t.module == module.name)
            .map(|t| s(typedef_key(&t.scope, &t.name)))
            .collect()),
    );
    m.insert(
        "identities".to_string(),
        arr(schema
            .identities()
            .filter(|i| i.module == module.name)
            .map(|i| s(qualified_name(&i.module, &i.name)))
            .collect()),
    );
    Ok(Json::Object(m))
}

fn flatten_subtree(root: NodeRef<'_>, out: &mut Map<String, Json>) -> R<()> {
    out.insert(root.path(), schema_node_entry(root)?);
    for child in root.children() {
        flatten_subtree(child, out)?;
    }
    Ok(())
}

fn index_node_sids(node: NodeRef<'_>, out: &mut Map<String, Json>) {
    if let Some(sid) = node.sid {
        out.insert(
            sid.to_string(),
            obj(vec![
                ("namespace", s("data")),
                ("identifier", s(node.path())),
            ]),
        );
    }
    for child in node.children() {
        index_node_sids(child, out);
    }
}

pub fn to_resolved_catalog(schema: &Schema, yang_catalog: &str) -> R<Json> {
    let mut modules = Map::new();
    for module in schema.modules() {
        modules.insert(module.name.clone(), module_entry(module, schema)?);
    }

    let mut typedefs = Map::new();
    for t in schema.typedefs() {
        typedefs.insert(typedef_key(&t.scope, &t.name), typedef_entry(t)?);
    }

    let mut identities = Map::new();
    for identity in schema.identities() {
        identities.insert(
            qualified_name(&identity.module, &identity.name),
            identity_entry(identity)?,
        );
    }

    let mut schema_nodes = Map::new();
    for module in schema.modules() {
        for root in module.root_nodes() {
            flatten_subtree(root, &mut schema_nodes)?;
        }
    }

    let mut yang_data = Map::new();
    for root in schema.yang_data_roots() {
        flatten_subtree(root, &mut yang_data)?;
    }

    let mut sid_index = Map::new();
    for module in schema.modules() {
        if let Some(sid) = module.sid {
            sid_index.insert(
                sid.to_string(),
                obj(vec![
                    ("namespace", s("module")),
                    ("identifier", s(module.name.clone())),
                ]),
            );
        }
    }
    for identity in schema.identities() {
        if let Some(sid) = identity.sid {
            sid_index.insert(
                sid.to_string(),
                obj(vec![
                    ("namespace", s("identity")),
                    (
                        "identifier",
                        s(qualified_name(&identity.module, &identity.name)),
                    ),
                ]),
            );
        }
    }
    for module in schema.modules() {
        for root in module.root_nodes() {
            index_node_sids(root, &mut sid_index);
        }
    }
    for root in schema.yang_data_roots() {
        index_node_sids(root, &mut sid_index);
    }

    let mut doc = Map::new();
    doc.insert("$schema".to_string(), s(SCHEMA_URL));
    doc.insert("schema-version".to_string(), s(SCHEMA_VERSION));
    doc.insert("yang-catalog".to_string(), s(yang_catalog));
    doc.insert("modules".to_string(), Json::Object(modules));
    doc.insert("typedefs".to_string(), Json::Object(typedefs));
    doc.insert("identities".to_string(), Json::Object(identities));
    doc.insert("schema-nodes".to_string(), Json::Object(schema_nodes));
    doc.insert("yang-data".to_string(), Json::Object(yang_data));
    doc.insert("sid-index".to_string(), Json::Object(sid_index));
    Ok(Json::Object(doc))
}
