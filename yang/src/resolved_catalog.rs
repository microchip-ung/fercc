// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

use serde_json::{Map, Value as Json};

use crate::json_schema::num;
use crate::schema::{
    default_ranges, Builtin, IdentityRef, ImportOnlyModule, ModuleRef, NodeRef, Schema, Status,
    TypeRef, TypedefRef,
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

fn sorted_strs(mut items: Vec<Json>) -> Vec<Json> {
    items.sort_by(|a, b| a.as_str().unwrap().cmp(b.as_str().unwrap()));
    items
}

fn sorted_map(m: Map<String, Json>) -> Map<String, Json> {
    let mut entries: Vec<(String, Json)> = m.into_iter().collect();
    entries.sort_by(|(a, _), (b, _)| a.cmp(b));
    entries.into_iter().collect()
}

fn sorted_map_numeric(m: Map<String, Json>) -> Map<String, Json> {
    let mut entries: Vec<(String, Json)> = m.into_iter().collect();
    entries.sort_by_key(|(k, _)| k.parse::<i64>().unwrap());
    entries.into_iter().collect()
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
    if scope.starts_with('/') {
        format!("{scope} {name}")
    } else {
        format!("{scope}:{name}")
    }
}

fn import_only_module_key(name: &str, import_revision_date: Option<&str>) -> String {
    match import_revision_date {
        Some(date) => format!("{name} {date}"),
        None => name.to_string(),
    }
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
            opt_insert(
                &mut m,
                "target",
                ty.leafref_target().map(|target| s(target.path())),
            );
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
        arr(sorted_strs(
            identity
                .derived()
                .filter(|d| d.sid.is_some())
                .map(|d| s(qualified_name(&d.module, &d.name)))
                .collect(),
        )),
    );
    Ok(Json::Object(m))
}

fn schema_node_entry(node: NodeRef<'_>) -> R<Json> {
    let mut m = Map::new();
    m.insert("keyword".to_string(), s(node.kw.clone()));
    m.insert("name".to_string(), s(node.local_name().to_string()));
    m.insert("module".to_string(), s(node.owner_module.clone()));
    opt_insert(&mut m, "parent", node.parent().map(|p| s(p.path())));
    if !matches!(
        node.kw.as_str(),
        "leaf" | "leaf-list" | "anydata" | "anyxml"
    ) {
        m.insert(
            "children".to_string(),
            arr(node.children().map(|c| s(c.path())).collect()),
        );
        m.insert(
            "data-children".to_string(),
            arr(node.data_children().map(|c| s(c.path())).collect()),
        );
    }
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
    m.insert("implicit".to_string(), Json::Bool(node.synthetic));

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
    m.insert(
        "yang-data".to_string(),
        arr(schema
            .yang_data()
            .filter(|y| y.module == module.name)
            .map(|y| s(qualified_name(&y.module, &y.name)))
            .collect()),
    );
    Ok(Json::Object(m))
}

fn import_only_module_entry(module: &ImportOnlyModule, schema: &Schema) -> R<Json> {
    let mut m = Map::new();
    m.insert("name".to_string(), s(module.name.clone()));
    m.insert("namespace".to_string(), s(module.namespace.clone()));
    m.insert(
        "sid".to_string(),
        s(sid_string(
            module.sid,
            &format!("import-only module {}", module.name),
        )?),
    );
    opt_insert(
        &mut m,
        "revision",
        module.import_revision_date.clone().map(s),
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

    let mut import_only_modules = Map::new();
    for module in schema.import_only_modules() {
        let key = import_only_module_key(&module.name, module.import_revision_date.as_deref());
        import_only_modules.insert(key, import_only_module_entry(module, schema)?);
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
    for template in schema.yang_data() {
        flatten_subtree(template.root(), &mut schema_nodes)?;
        yang_data.insert(
            qualified_name(&template.module, &template.name),
            obj(vec![
                ("module", s(template.module.clone())),
                ("name", s(template.name.clone())),
                ("root", s(template.root().path())),
            ]),
        );
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
    for module in schema.import_only_modules() {
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
    for template in schema.yang_data() {
        index_node_sids(template.root(), &mut sid_index);
    }

    let mut doc = Map::new();
    doc.insert("$schema".to_string(), s(SCHEMA_URL));
    doc.insert("schema-version".to_string(), s(SCHEMA_VERSION));
    doc.insert("yang-catalog".to_string(), s(yang_catalog));
    doc.insert("modules".to_string(), Json::Object(sorted_map(modules)));
    doc.insert(
        "import-only-modules".to_string(),
        Json::Object(sorted_map(import_only_modules)),
    );
    doc.insert("typedefs".to_string(), Json::Object(sorted_map(typedefs)));
    doc.insert(
        "identities".to_string(),
        Json::Object(sorted_map(identities)),
    );
    doc.insert(
        "schema-nodes".to_string(),
        Json::Object(sorted_map(schema_nodes)),
    );
    doc.insert("yang-data".to_string(), Json::Object(sorted_map(yang_data)));
    doc.insert(
        "sid-index".to_string(),
        Json::Object(sorted_map_numeric(sid_index)),
    );
    verify_resolved_catalog(&doc)?;
    Ok(Json::Object(doc))
}

fn collect_derived_from(ty: &Json, out: &mut std::collections::HashSet<String>) {
    if let Some(key) = ty.get("derived-from").and_then(Json::as_str) {
        out.insert(key.to_string());
    }
    if let Some(members) = ty.get("type").and_then(Json::as_array) {
        for member in members {
            collect_derived_from(member, out);
        }
    }
}

fn collect_identityref_bases(ty: &Json, out: &mut std::collections::HashSet<String>) {
    if ty.get("builtin").and_then(Json::as_str) == Some("identityref") {
        if let Some(bases) = ty.get("base").and_then(Json::as_array) {
            for b in bases {
                out.insert(b.as_str().unwrap().to_string());
            }
        }
    }
    if let Some(members) = ty.get("type").and_then(Json::as_array) {
        for member in members {
            collect_identityref_bases(member, out);
        }
    }
}

fn check_type_descriptor(
    ty: &Json,
    typedefs: &Map<String, Json>,
    schema_nodes: &Map<String, Json>,
    from: &str,
    errors: &mut Vec<String>,
) {
    if let Some(key) = ty.get("derived-from").and_then(Json::as_str) {
        if !typedefs.contains_key(key) {
            errors.push(format!(
                "{from}.type.derived-from: {key:?} not found in typedefs"
            ));
        }
    }
    if let Some(target) = ty.get("target").and_then(Json::as_str) {
        if !schema_nodes.contains_key(target) {
            errors.push(format!(
                "{from}.type.target: {target:?} not found in schema-nodes"
            ));
        }
    }
    if let Some(members) = ty.get("type").and_then(Json::as_array) {
        for member in members {
            check_type_descriptor(member, typedefs, schema_nodes, from, errors);
        }
    }
}

fn verify_resolved_catalog(doc: &Map<String, Json>) -> R<()> {
    let mut errors = Vec::new();

    let schema_nodes = doc["schema-nodes"].as_object().unwrap();
    let yang_data = doc["yang-data"].as_object().unwrap();
    let typedefs = doc["typedefs"].as_object().unwrap();
    let identities = doc["identities"].as_object().unwrap();
    let modules = doc["modules"].as_object().unwrap();
    let import_only_modules = doc["import-only-modules"].as_object().unwrap();
    let sid_index = doc["sid-index"].as_object().unwrap();

    for (path, node) in schema_nodes {
        let from = format!("schema-nodes[{path:?}]");
        if let Some(parent) = node.get("parent").and_then(Json::as_str) {
            if !schema_nodes.contains_key(parent) {
                errors.push(format!(
                    "{from}.parent: {parent:?} not found in schema-nodes"
                ));
            } else if !schema_nodes[parent]
                .get("children")
                .and_then(Json::as_array)
                .is_some_and(|children| children.iter().any(|c| c.as_str() == Some(path.as_str())))
            {
                errors.push(format!(
                    "{from}.parent is {parent:?}, but {parent:?}.children does not list {path:?} back"
                ));
            }
        }
        for field in ["children", "data-children", "keys"] {
            if let Some(items) = node.get(field).and_then(Json::as_array) {
                for item in items {
                    let p = item.as_str().unwrap();
                    if !schema_nodes.contains_key(p) {
                        errors.push(format!("{from}.{field}: {p:?} not found in schema-nodes"));
                    } else if field == "children"
                        && schema_nodes[p].get("parent").and_then(Json::as_str)
                            != Some(path.as_str())
                    {
                        errors.push(format!(
                            "{from}.children lists {p:?}, but {p:?}.parent is not {path:?}"
                        ));
                    }
                }
            }
        }
        if let Some(t) = node.get("type") {
            check_type_descriptor(t, typedefs, schema_nodes, &from, &mut errors);
        }
    }

    for (key, t) in typedefs {
        let from = format!("typedefs[{key:?}]");
        check_type_descriptor(&t["type"], typedefs, schema_nodes, &from, &mut errors);
    }

    for (key, identity) in identities {
        let from = format!("identities[{key:?}]");
        for field in ["base", "derived"] {
            for item in identity[field].as_array().unwrap() {
                let name = item.as_str().unwrap();
                if !identities.contains_key(name) {
                    errors.push(format!("{from}.{field}: {name:?} not found in identities"));
                }
            }
        }
    }

    for (mod_name, module) in modules {
        let from = format!("modules[{mod_name:?}]");
        for field in ["root-nodes", "data-root-nodes"] {
            for item in module[field].as_array().unwrap() {
                let p = item.as_str().unwrap();
                if !schema_nodes.contains_key(p) {
                    errors.push(format!("{from}.{field}: {p:?} not found in schema-nodes"));
                }
            }
        }
        for item in module["typedefs"].as_array().unwrap() {
            let key = item.as_str().unwrap();
            if !typedefs.contains_key(key) {
                errors.push(format!("{from}.typedefs: {key:?} not found in typedefs"));
            }
        }
        for item in module["identities"].as_array().unwrap() {
            let key = item.as_str().unwrap();
            if !identities.contains_key(key) {
                errors.push(format!(
                    "{from}.identities: {key:?} not found in identities"
                ));
            }
        }
        for item in module["yang-data"].as_array().unwrap() {
            let key = item.as_str().unwrap();
            match yang_data.get(key) {
                None => errors.push(format!("{from}.yang-data: {key:?} not found in yang-data")),
                Some(template) if template["module"] != *mod_name => errors.push(format!(
                    "{from}.yang-data lists {key:?}, but its module is {}",
                    template["module"]
                )),
                Some(_) => {}
            }
        }
    }

    for (key, template) in yang_data {
        let from = format!("yang-data[{key:?}]");
        let module = template["module"].as_str().unwrap();
        let name = template["name"].as_str().unwrap();
        if *key != qualified_name(module, name) {
            errors.push(format!(
                "{from}'s key doesn't match its own \"module\" and \"name\" fields"
            ));
        }
        let listed_by_its_module = modules
            .get(module)
            .and_then(|m| m["yang-data"].as_array())
            .is_some_and(|list| list.iter().any(|k| k.as_str() == Some(key.as_str())));
        if !listed_by_its_module {
            errors.push(format!(
                "{from} is not listed by modules[{module:?}].yang-data"
            ));
        }
        let root = template["root"].as_str().unwrap();
        match schema_nodes.get(root) {
            None => errors.push(format!("{from}.root: {root:?} not found in schema-nodes")),
            Some(node) => {
                if node["keyword"] != "container" {
                    errors.push(format!("{from}.root {root:?} is not a container"));
                }
                if node["module"] != *module {
                    errors.push(format!("{from}.root {root:?} is not defined in {module:?}"));
                }
            }
        }
    }

    for (key, module) in import_only_modules {
        let from = format!("import-only-modules[{key:?}]");
        for item in module["typedefs"].as_array().unwrap() {
            let key = item.as_str().unwrap();
            if !typedefs.contains_key(key) {
                errors.push(format!("{from}.typedefs: {key:?} not found in typedefs"));
            }
        }
        for item in module["identities"].as_array().unwrap() {
            let key = item.as_str().unwrap();
            if !identities.contains_key(key) {
                errors.push(format!(
                    "{from}.identities: {key:?} not found in identities"
                ));
            }
        }
        let module_name = module["name"].as_str().unwrap();
        let key_name = key.split(' ').next().unwrap();
        if key_name != module_name {
            errors.push(format!(
                "import-only-modules[{key:?}]'s key name doesn't match its own \"name\" field"
            ));
        }
    }

    for (sid, entry) in sid_index {
        let namespace = entry["namespace"].as_str().unwrap();
        let identifier = entry["identifier"].as_str().unwrap();
        let found = match namespace {
            "module" => {
                modules.contains_key(identifier)
                    || import_only_modules
                        .values()
                        .any(|m| m["name"] == *identifier)
            }
            "identity" => identities.contains_key(identifier),
            "data" => schema_nodes.contains_key(identifier),
            other => {
                errors.push(format!("sid-index[{sid:?}]: unknown namespace {other:?}"));
                continue;
            }
        };
        if !found {
            errors.push(format!(
                "sid-index[{sid:?}]: {namespace}:{identifier:?} not found"
            ));
        }
    }

    for (mod_name, module) in modules {
        let sid = module["sid"].as_str().unwrap();
        if sid_index.get(sid).map(|e| e["identifier"].as_str()) != Some(Some(mod_name.as_str())) {
            errors.push(format!(
                "module {mod_name:?}'s sid {sid:?} not reverse-indexed"
            ));
        }
    }
    for (key, module) in import_only_modules {
        let sid = module["sid"].as_str().unwrap();
        let name = module["name"].as_str().unwrap();
        if sid_index.get(sid).map(|e| e["identifier"].as_str()) != Some(Some(name)) {
            errors.push(format!(
                "import-only-modules[{key:?}]'s sid {sid:?} not reverse-indexed"
            ));
        }
    }
    for (key, identity) in identities {
        let sid = identity["sid"].as_str().unwrap();
        if sid_index.get(sid).map(|e| e["identifier"].as_str()) != Some(Some(key.as_str())) {
            errors.push(format!(
                "identity {key:?}'s sid {sid:?} not reverse-indexed"
            ));
        }
    }
    for (path, node) in schema_nodes {
        let Some(sid) = node.get("sid").and_then(Json::as_str) else {
            continue;
        };
        if sid_index.get(sid).map(|e| e["identifier"].as_str()) != Some(Some(path.as_str())) {
            errors.push(format!(
                "schema-nodes[{path:?}]'s sid {sid:?} not reverse-indexed"
            ));
        }
    }

    let mut sid_bearing = schema_nodes
        .values()
        .filter(|n| n.get("sid").is_some())
        .count();
    sid_bearing += identities.len();
    let mut module_sids: std::collections::HashSet<String> = modules
        .values()
        .filter_map(|m| m.get("sid").and_then(Json::as_str))
        .map(str::to_string)
        .collect();
    module_sids.extend(
        import_only_modules
            .values()
            .filter_map(|m| m.get("sid").and_then(Json::as_str))
            .map(str::to_string),
    );
    sid_bearing += module_sids.len();
    if sid_index.len() != sid_bearing {
        errors.push(format!(
            "sid-index has {} entries but {sid_bearing} sid-bearing entities exist in the document",
            sid_index.len()
        ));
    }

    let mut visited = std::collections::HashSet::new();
    let mut stack: Vec<&str> = schema_nodes
        .iter()
        .filter(|(_, node)| node.get("parent").is_none())
        .map(|(path, _)| path.as_str())
        .collect();
    while let Some(path) = stack.pop() {
        if visited.insert(path.to_string()) {
            if let Some(children) = schema_nodes[path].get("children").and_then(Json::as_array) {
                for child in children {
                    stack.push(child.as_str().unwrap());
                }
            }
        }
    }
    for path in schema_nodes.keys() {
        if !visited.contains(path.as_str()) {
            errors.push(format!(
                "schema-nodes[{path:?}] is unreachable by walking children from a root"
            ));
        }
    }

    let mut root_listings: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    for module in modules.values() {
        for item in module["root-nodes"].as_array().unwrap() {
            *root_listings.entry(item.as_str().unwrap()).or_default() += 1;
        }
    }
    for template in yang_data.values() {
        *root_listings
            .entry(template["root"].as_str().unwrap())
            .or_default() += 1;
    }
    for (path, node) in schema_nodes {
        let listings = root_listings.get(path.as_str()).copied().unwrap_or(0);
        let has_parent = node.get("parent").is_some();
        if has_parent && listings != 0 {
            errors.push(format!(
                "schema-nodes[{path:?}] carries a parent but is listed as a module root-node or a yang-data root"
            ));
        }
        if !has_parent && listings != 1 {
            errors.push(format!(
                "schema-nodes[{path:?}] carries no parent, so must be listed exactly once as a module root-node or a yang-data root, but is listed {listings} times"
            ));
        }
    }

    let mut derived_from_refs = std::collections::HashSet::new();
    let mut identityref_base_refs = std::collections::HashSet::new();
    for node in schema_nodes.values() {
        if let Some(ty) = node.get("type") {
            collect_derived_from(ty, &mut derived_from_refs);
            collect_identityref_bases(ty, &mut identityref_base_refs);
        }
    }
    for t in typedefs.values() {
        collect_derived_from(&t["type"], &mut derived_from_refs);
    }
    for key in typedefs.keys() {
        if !derived_from_refs.contains(key) {
            errors.push(format!(
                "typedefs[{key:?}] is never named by any derived-from in the document"
            ));
        }
    }
    let mut reachable_identities = std::collections::HashSet::new();
    let mut stack: Vec<String> = identityref_base_refs.into_iter().collect();
    while let Some(key) = stack.pop() {
        if reachable_identities.insert(key.clone()) {
            if let Some(identity) = identities.get(&key) {
                for d in identity["derived"].as_array().unwrap() {
                    stack.push(d.as_str().unwrap().to_string());
                }
            }
        }
    }
    for key in identities.keys() {
        if !reachable_identities.contains(key) {
            errors.push(format!(
                "identities[{key:?}] is never named as a base (directly or via derivation) by any identityref in the document"
            ));
        }
    }

    let actual_modules: std::collections::HashSet<&str> =
        modules.keys().map(|k| k.as_str()).collect();
    let actual_import_only: std::collections::HashSet<&str> = import_only_modules
        .values()
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    for name in actual_modules.intersection(&actual_import_only) {
        errors.push(format!(
            "{name:?} appears in both modules and import-only-modules"
        ));
    }
    for (key, module) in import_only_modules {
        if module.get("root-nodes").is_some() {
            errors.push(format!("import-only-modules[{key:?}] has root-nodes"));
        }
        if module.get("data-root-nodes").is_some() {
            errors.push(format!("import-only-modules[{key:?}] has data-root-nodes"));
        }
    }

    let mut namespace_owners: std::collections::HashMap<&str, &str> =
        std::collections::HashMap::new();
    for (name, m) in modules {
        namespace_owners.insert(m["namespace"].as_str().unwrap(), name.as_str());
    }
    for (key, entry) in import_only_modules {
        let module_name = entry["name"].as_str().unwrap();
        let namespace = entry["namespace"].as_str().unwrap();
        if let Some(other) = namespace_owners.get(namespace) {
            if *other != module_name {
                errors.push(format!(
                    "import-only-modules[{key:?}]'s namespace {namespace:?} collides with {other:?}"
                ));
            }
        } else {
            namespace_owners.insert(namespace, module_name);
        }
    }

    for (path, node) in schema_nodes {
        if matches!(
            node.get("keyword").and_then(Json::as_str),
            Some("leaf" | "leaf-list" | "anydata" | "anyxml")
        ) {
            if node.get("children").is_some() {
                errors.push(format!(
                    "schema-nodes[{path:?}] is a leaf/leaf-list/anydata/anyxml but carries a \"children\" field"
                ));
            }
            if node.get("data-children").is_some() {
                errors.push(format!(
                    "schema-nodes[{path:?}] is a leaf/leaf-list/anydata/anyxml but carries a \"data-children\" field"
                ));
            }
        }
    }

    for (key, t) in typedefs {
        if t["type"].get("builtin").and_then(Json::as_str) == Some("leafref")
            && t["type"].get("target").is_some()
        {
            errors.push(format!(
                "typedefs[{key:?}]'s own type descriptor must never carry a resolved leafref target"
            ));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "resolved catalog has {} consistency error(s):\n{}",
            errors.len(),
            errors.join("\n")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typedef_key_uses_module_colon_name_for_a_top_level_typedef_but_space_form_when_nested() {
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
                typedef inner-percent {
                  type uint8 {
                    range "0..50";
                  }
                }
                leaf a {
                  type percent;
                }
                leaf b {
                  type inner-percent;
                }
              }
            }
        "#;
        let sid_src = r#"{
            "module-name": "foo",
            "module-revision": "2026-01-01",
            "items": [
                {"namespace": "module", "identifier": "foo", "sid": 1000},
                {"namespace": "data", "identifier": "/foo:bar", "sid": 1001},
                {"namespace": "data", "identifier": "/foo:bar/a", "sid": 1002},
                {"namespace": "data", "identifier": "/foo:bar/b", "sid": 1003}
            ]
        }"#;
        let schema =
            crate::schema::build(&[src.to_string()], &[sid_src.to_string()], &[], &[]).unwrap();
        let doc = to_resolved_catalog(&schema, "deadbeef").unwrap();

        assert!(doc["typedefs"].get("foo:percent").is_some());
        assert!(doc["typedefs"].get("/foo:bar inner-percent").is_some());
        assert_eq!(
            doc["modules"]["foo"]["typedefs"],
            serde_json::json!(["foo:percent", "/foo:bar inner-percent"])
        );

        let schema_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("schema")
            .join("resolved-yang-catalog-schema-v1.0.0.json");
        let schema_text = std::fs::read_to_string(schema_path).unwrap();
        let json_schema: Json = serde_json::from_str(&schema_text).unwrap();
        let validator = jsonschema::validator_for(&json_schema).unwrap();
        let errors: Vec<_> = validator.iter_errors(&doc).collect();
        assert!(
            errors.is_empty(),
            "document fails schema validation: {errors:?}"
        );
    }

    fn yang_data_doc() -> Map<String, Json> {
        let src = r#"
            module foo {
              namespace "urn:foo";
              prefix f;

              rc:yang-data error-template {
                container error {
                  leaf message {
                    type string;
                  }
                }
              }
            }
        "#;
        let sid_src = r#"{
            "module-name": "foo",
            "module-revision": "2026-01-01",
            "items": [
                {"namespace": "module", "identifier": "foo", "sid": 1000},
                {"namespace": "data", "identifier": "/foo:error", "sid": 1001},
                {"namespace": "data", "identifier": "/foo:error/message", "sid": 1002}
            ]
        }"#;
        let schema = crate::schema::build(
            &[src.to_string()],
            &[sid_src.to_string()],
            &["foo:error-template"],
            &[],
        )
        .unwrap();
        let Json::Object(doc) = to_resolved_catalog(&schema, "deadbeef").unwrap() else {
            unreachable!()
        };
        doc
    }

    fn verify_error(doc: &Map<String, Json>) -> String {
        verify_resolved_catalog(doc).expect_err("the corrupted document must fail verification")
    }

    #[test]
    fn a_yang_data_templates_root_lives_in_schema_nodes_and_is_listed_by_its_module() {
        let doc = yang_data_doc();
        assert_eq!(
            doc["yang-data"],
            serde_json::json!({
                "foo:error-template": {
                    "module": "foo",
                    "name": "error-template",
                    "root": "/foo:error"
                }
            })
        );
        assert_eq!(
            doc["modules"]["foo"]["yang-data"],
            serde_json::json!(["foo:error-template"])
        );
        assert_eq!(doc["modules"]["foo"]["root-nodes"], serde_json::json!([]));
        assert!(doc["schema-nodes"]["/foo:error"].get("parent").is_none());
        assert_eq!(
            doc["schema-nodes"]["/foo:error/message"]["parent"],
            "/foo:error"
        );
    }

    #[test]
    fn verify_rejects_a_parentless_node_that_is_neither_a_module_root_nor_a_yang_data_root() {
        let mut doc = yang_data_doc();
        doc["yang-data"].as_object_mut().unwrap().clear();
        doc["modules"]["foo"]["yang-data"] = serde_json::json!([]);
        let e = verify_error(&doc);
        assert!(
            e.contains("schema-nodes[\"/foo:error\"] carries no parent"),
            "{e}"
        );
    }

    #[test]
    fn verify_rejects_a_parentless_node_that_is_both_a_module_root_and_a_yang_data_root() {
        let mut doc = yang_data_doc();
        doc["modules"]["foo"]["root-nodes"] = serde_json::json!(["/foo:error"]);
        doc["modules"]["foo"]["data-root-nodes"] = serde_json::json!(["/foo:error"]);
        let e = verify_error(&doc);
        assert!(e.contains("listed 2 times"), "{e}");
    }

    #[test]
    fn verify_rejects_a_yang_data_root_that_carries_a_parent() {
        let mut doc = yang_data_doc();
        doc["yang-data"]["foo:error-template"]["root"] = serde_json::json!("/foo:error/message");
        let e = verify_error(&doc);
        assert!(
            e.contains("schema-nodes[\"/foo:error/message\"] carries a parent but is listed"),
            "{e}"
        );
        assert!(e.contains("is not a container"), "{e}");
    }

    #[test]
    fn verify_rejects_a_yang_data_root_missing_from_schema_nodes() {
        let mut doc = yang_data_doc();
        doc["yang-data"]["foo:error-template"]["root"] = serde_json::json!("/foo:nowhere");
        let e = verify_error(&doc);
        assert!(
            e.contains("yang-data[\"foo:error-template\"].root: \"/foo:nowhere\" not found in schema-nodes"),
            "{e}"
        );
    }

    #[test]
    fn verify_rejects_a_yang_data_key_that_disagrees_with_its_module_and_name() {
        let mut doc = yang_data_doc();
        doc["yang-data"]["foo:error-template"]["name"] = serde_json::json!("other");
        let e = verify_error(&doc);
        assert!(e.contains("key doesn't match"), "{e}");
    }

    #[test]
    fn verify_rejects_a_yang_data_template_its_module_does_not_list() {
        let mut doc = yang_data_doc();
        doc["modules"]["foo"]["yang-data"] = serde_json::json!([]);
        let e = verify_error(&doc);
        assert!(
            e.contains(
                "yang-data[\"foo:error-template\"] is not listed by modules[\"foo\"].yang-data"
            ),
            "{e}"
        );
    }

    #[test]
    fn verify_rejects_a_module_listing_a_yang_data_template_that_does_not_exist() {
        let mut doc = yang_data_doc();
        doc["modules"]["foo"]["yang-data"] =
            serde_json::json!(["foo:error-template", "foo:missing"]);
        let e = verify_error(&doc);
        assert!(
            e.contains("modules[\"foo\"].yang-data: \"foo:missing\" not found in yang-data"),
            "{e}"
        );
    }

    #[test]
    fn verify_rejects_a_module_listing_another_modules_yang_data_template() {
        let mut doc = yang_data_doc();
        doc["yang-data"]["foo:error-template"]["module"] = serde_json::json!("bar");
        let e = verify_error(&doc);
        assert!(e.contains("but its module is \"bar\""), "{e}");
        assert!(e.contains("is not defined in \"bar\""), "{e}");
    }
}
