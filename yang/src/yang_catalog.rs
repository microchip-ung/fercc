// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

use serde_json::{Map, Value as Json};

use crate::json_schema::num;
use crate::schema::{
    default_ranges, Builtin, ErrorInfo, IdentityRef, ImportOnlyModule, ModuleRef, NodeRef, Schema,
    Status, TypeRef, TypedefRef,
};

const SCHEMA_URL: &str = "urn:fercc:yang-catalog-schema:1.0.0";

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

fn with_error_info(mut m: Map<String, Json>, error: &ErrorInfo) -> Json {
    opt_insert(&mut m, "error-message", error.error_message.clone().map(s));
    opt_insert(&mut m, "error-app-tag", error.error_app_tag.clone().map(s));
    Json::Object(m)
}

fn range_object(ty: TypeRef<'_>) -> Json {
    let mut m = Map::new();
    m.insert("bounds".to_string(), range_bounds(&ty.ranges));
    with_error_info(m, &ty.range_error)
}

fn range64_object(ty: TypeRef<'_>) -> Json {
    let mut m = Map::new();
    m.insert("bounds".to_string(), range64_bounds(&ty.ranges));
    with_error_info(m, &ty.range_error)
}

fn is_explicitly_restricted(ty: TypeRef<'_>) -> bool {
    ty.ranges != default_ranges(ty.builtin)
}

fn type_descriptor(ty: TypeRef<'_>) -> R<Json> {
    let mut m = Map::new();
    m.insert("builtin".to_string(), s(builtin_str(ty.builtin)));
    opt_insert(
        &mut m,
        "base-type",
        ty.base()
            .map(|base| s(typedef_key(&base.scope, &base.name))),
    );

    match ty.builtin {
        Builtin::Binary => {
            if is_explicitly_restricted(ty) {
                m.insert("length".to_string(), range_object(ty));
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
                m.insert("range".to_string(), range64_object(ty));
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
            opt_insert(
                &mut m,
                "values",
                ty.identity_values().map(|values| {
                    arr(sorted_strs(
                        values
                            .into_iter()
                            .map(|v| s(qualified_name(&v.module, &v.name)))
                            .collect(),
                    ))
                }),
            );
        }
        Builtin::InstanceIdentifier => {
            m.insert(
                "require-instance".to_string(),
                Json::Bool(ty.require_instance),
            );
        }
        Builtin::Int8
        | Builtin::Int16
        | Builtin::Int32
        | Builtin::Uint8
        | Builtin::Uint16
        | Builtin::Uint32 => {
            if is_explicitly_restricted(ty) {
                m.insert("range".to_string(), range_object(ty));
            }
        }
        Builtin::Int64 | Builtin::Uint64 => {
            if is_explicitly_restricted(ty) {
                m.insert("range".to_string(), range64_object(ty));
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
            m.insert(
                "require-instance".to_string(),
                Json::Bool(ty.require_instance),
            );
        }
        Builtin::String => {
            if is_explicitly_restricted(ty) {
                m.insert("length".to_string(), range_object(ty));
            }
            if !ty.patterns.is_empty() {
                m.insert(
                    "pattern".to_string(),
                    arr(ty
                        .patterns
                        .iter()
                        .map(|p| {
                            let mut m = Map::new();
                            m.insert("pattern".to_string(), s(p.regex.clone()));
                            m.insert("invert-match".to_string(), Json::Bool(p.invert_match));
                            with_error_info(m, &p.error)
                        })
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
    opt_insert(&mut m, "default", t.default().map(|d| s(d.to_string())));
    opt_insert(&mut m, "units", t.units().map(|u| s(u.to_string())));
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
            "schema-children".to_string(),
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
            opt_insert(&mut m, "default", node.defaults.first().cloned().map(s));
            opt_insert(&mut m, "units", node.units.clone().map(s));
        }
        "leaf-list" => {
            m.insert("config".to_string(), Json::Bool(node.config));
            let ty = node
                .ty()
                .ok_or_else(|| format!("leaf-list {} has no type", node.path()))?;
            m.insert("type".to_string(), type_descriptor(ty)?);
            if !node.defaults.is_empty() {
                m.insert(
                    "default".to_string(),
                    arr(node.defaults.iter().cloned().map(s).collect()),
                );
            }
            opt_insert(&mut m, "units", node.units.clone().map(s));
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
        "schema-root-nodes".to_string(),
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
            .filter(|i| i.module == module.name && i.is_implemented())
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
    m.insert(
        "deviations".to_string(),
        arr(sorted_strs(
            module.deviated_by.iter().map(|d| s(d.clone())).collect(),
        )),
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

pub fn to_yang_catalog(schema: &Schema) -> R<Json> {
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
    verify_yang_catalog(&doc)?;
    Ok(Json::Object(doc))
}

fn collect_base_types(ty: &Json, out: &mut std::collections::HashSet<String>) {
    if let Some(key) = ty.get("base-type").and_then(Json::as_str) {
        out.insert(key.to_string());
    }
    if let Some(members) = ty.get("type").and_then(Json::as_array) {
        for member in members {
            collect_base_types(member, out);
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

fn collect_identityref_values(ty: &Json, out: &mut std::collections::HashSet<String>) {
    if let Some(values) = ty.get("values").and_then(Json::as_array) {
        for v in values {
            out.insert(v.as_str().unwrap().to_string());
        }
    }
    if let Some(members) = ty.get("type").and_then(Json::as_array) {
        for member in members {
            collect_identityref_values(member, out);
        }
    }
}

fn check_identityref_values(
    ty: &Json,
    identities: &Map<String, Json>,
    from: &str,
    errors: &mut Vec<String>,
) {
    if ty.get("builtin").and_then(Json::as_str) == Some("identityref") {
        match ty.get("values").and_then(Json::as_array) {
            None => errors.push(format!("{from}.type: an identityref has no \"values\"")),
            Some(values) => {
                let names: Vec<&str> = values.iter().map(|v| v.as_str().unwrap()).collect();
                if !names.windows(2).all(|w| w[0] < w[1]) {
                    errors.push(format!(
                        "{from}.type.values is not sorted alphabetically without duplicates"
                    ));
                }
                let bases: Vec<&str> = ty["base"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|b| b.as_str().unwrap())
                    .collect();
                for name in names {
                    if !identities.contains_key(name) {
                        errors.push(format!(
                            "{from}.type.values: {name:?} not found in identities"
                        ));
                    } else if !bases.iter().all(|b| derives_from(identities, name, b)) {
                        errors.push(format!(
                            "{from}.type.values: {name:?} is not derived from every base in {bases:?}"
                        ));
                    }
                }
            }
        }
    }
    if let Some(members) = ty.get("type").and_then(Json::as_array) {
        for member in members {
            check_identityref_values(member, identities, from, errors);
        }
    }
}

fn derives_from(identities: &Map<String, Json>, identity: &str, base: &str) -> bool {
    let mut stack: Vec<&str> = vec![identity];
    let mut seen = std::collections::HashSet::new();
    while let Some(id) = stack.pop() {
        if let Some(entry) = identities.get(id) {
            for b in entry["base"].as_array().unwrap() {
                let b = b.as_str().unwrap();
                if b == base {
                    return true;
                }
                if seen.insert(b) {
                    stack.push(b);
                }
            }
        }
    }
    false
}

fn check_type_descriptor(
    ty: &Json,
    typedefs: &Map<String, Json>,
    schema_nodes: &Map<String, Json>,
    from: &str,
    errors: &mut Vec<String>,
) {
    if let Some(key) = ty.get("base-type").and_then(Json::as_str) {
        if !typedefs.contains_key(key) {
            errors.push(format!(
                "{from}.type.base-type: {key:?} not found in typedefs"
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

fn verify_yang_catalog(doc: &Map<String, Json>) -> R<()> {
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
                .get("schema-children")
                .and_then(Json::as_array)
                .is_some_and(|children| children.iter().any(|c| c.as_str() == Some(path.as_str())))
            {
                errors.push(format!(
                    "{from}.parent is {parent:?}, but {parent:?}.schema-children does not list {path:?} back"
                ));
            }
        }
        for field in ["schema-children", "data-children", "keys"] {
            if let Some(items) = node.get(field).and_then(Json::as_array) {
                for item in items {
                    let p = item.as_str().unwrap();
                    if !schema_nodes.contains_key(p) {
                        errors.push(format!("{from}.{field}: {p:?} not found in schema-nodes"));
                    } else if field == "schema-children"
                        && schema_nodes[p].get("parent").and_then(Json::as_str)
                            != Some(path.as_str())
                    {
                        errors.push(format!(
                            "{from}.schema-children lists {p:?}, but {p:?}.parent is not {path:?}"
                        ));
                    }
                }
            }
        }
        if let Some(t) = node.get("type") {
            check_type_descriptor(t, typedefs, schema_nodes, &from, &mut errors);
            check_identityref_values(t, identities, &from, &mut errors);
        }
    }

    for (key, t) in typedefs {
        let from = format!("typedefs[{key:?}]");
        check_type_descriptor(&t["type"], typedefs, schema_nodes, &from, &mut errors);
    }

    for (key, identity) in identities {
        let from = format!("identities[{key:?}]");
        for item in identity["base"].as_array().unwrap() {
            let name = item.as_str().unwrap();
            if !identities.contains_key(name) {
                errors.push(format!("{from}.base: {name:?} not found in identities"));
            }
        }
    }

    for (mod_name, module) in modules {
        let from = format!("modules[{mod_name:?}]");
        for field in ["schema-root-nodes", "data-root-nodes"] {
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
        let deviations = module["deviations"].as_array().unwrap();
        for item in deviations {
            let name = item.as_str().unwrap();
            if !modules.contains_key(name) {
                errors.push(format!("{from}.deviations: {name:?} not found in modules"));
            }
            if name == mod_name {
                errors.push(format!("{from}.deviations lists the module itself"));
            }
        }
        let deviation_names: Vec<&str> = deviations.iter().map(|d| d.as_str().unwrap()).collect();
        if !deviation_names.windows(2).all(|w| w[0] < w[1]) {
            errors.push(format!(
                "{from}.deviations is not sorted alphabetically without duplicates"
            ));
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

    let mut identity_listings: std::collections::HashMap<&str, Vec<String>> =
        std::collections::HashMap::new();
    for (mod_name, module) in modules {
        for item in module["identities"].as_array().unwrap() {
            identity_listings
                .entry(item.as_str().unwrap())
                .or_default()
                .push(format!("modules[{mod_name:?}]"));
        }
    }
    let mut import_only_listings: std::collections::HashMap<&str, std::collections::HashSet<&str>> =
        std::collections::HashMap::new();
    for module in import_only_modules.values() {
        let name = module["name"].as_str().unwrap();
        for item in module["identities"].as_array().unwrap() {
            import_only_listings
                .entry(item.as_str().unwrap())
                .or_default()
                .insert(name);
        }
    }
    for (key, names) in import_only_listings {
        for name in names {
            identity_listings
                .entry(key)
                .or_default()
                .push(format!("import-only-modules[{name:?}]"));
        }
    }
    for (key, identity) in identities {
        let listings = identity_listings
            .get(key.as_str())
            .cloned()
            .unwrap_or_default();
        let module = identity["module"].as_str().unwrap();
        if listings.len() > 1 {
            errors.push(format!(
                "identities[{key:?}] is listed by more than one module: {}",
                listings.join(", ")
            ));
        }
        for listing in &listings {
            if *listing != format!("modules[{module:?}]")
                && *listing != format!("import-only-modules[{module:?}]")
            {
                errors.push(format!(
                    "identities[{key:?}] is listed by {listing}, but is declared in {module:?}"
                ));
            }
        }
        if listings.is_empty() && !modules.contains_key(module) {
            errors.push(format!(
                "identities[{key:?}] is listed by no module, but only an implemented module's base identities may be unlisted"
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
            if let Some(children) = schema_nodes[path]
                .get("schema-children")
                .and_then(Json::as_array)
            {
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
        for item in module["schema-root-nodes"].as_array().unwrap() {
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
                "schema-nodes[{path:?}] carries a parent but is listed as a module schema-root-node or a yang-data root"
            ));
        }
        if !has_parent && listings != 1 {
            errors.push(format!(
                "schema-nodes[{path:?}] carries no parent, so must be listed exactly once as a module schema-root-node or a yang-data root, but is listed {listings} times"
            ));
        }
    }

    let mut base_type_refs = std::collections::HashSet::new();
    let mut identityref_base_refs = std::collections::HashSet::new();
    for node in schema_nodes.values() {
        if let Some(ty) = node.get("type") {
            collect_base_types(ty, &mut base_type_refs);
            collect_identityref_bases(ty, &mut identityref_base_refs);
        }
    }
    for t in typedefs.values() {
        collect_base_types(&t["type"], &mut base_type_refs);
    }
    for key in typedefs.keys() {
        if !base_type_refs.contains(key) {
            errors.push(format!(
                "typedefs[{key:?}] is never named by any base-type in the document"
            ));
        }
    }
    let implemented_identities: std::collections::HashSet<&str> = modules
        .values()
        .flat_map(|m| m["identities"].as_array().unwrap())
        .map(|i| i.as_str().unwrap())
        .collect();
    let mut bases_of_implemented = std::collections::HashSet::new();
    let mut stack: Vec<&str> = implemented_identities
        .iter()
        .copied()
        .chain(identityref_base_refs.iter().map(String::as_str))
        .collect();
    while let Some(key) = stack.pop() {
        if let Some(identity) = identities.get(key) {
            for b in identity["base"].as_array().unwrap() {
                let b = b.as_str().unwrap();
                if bases_of_implemented.insert(b) {
                    stack.push(b);
                }
            }
        }
    }
    for key in identities.keys() {
        let key = key.as_str();
        if !implemented_identities.contains(key)
            && !bases_of_implemented.contains(key)
            && !identityref_base_refs.contains(key)
        {
            errors.push(format!(
                "identities[{key:?}] is neither implemented nor referred to as a base by anything implemented"
            ));
        }
    }
    for module in import_only_modules.values() {
        for item in module["identities"].as_array().unwrap() {
            let key = item.as_str().unwrap();
            if !bases_of_implemented.contains(key) && !identityref_base_refs.contains(key) {
                errors.push(format!(
                    "import-only-modules[{:?}].identities lists {key:?}, which nothing implemented refers to as a base",
                    module["name"].as_str().unwrap()
                ));
            }
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
        if module.get("schema-root-nodes").is_some() {
            errors.push(format!(
                "import-only-modules[{key:?}] has schema-root-nodes"
            ));
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
            if node.get("schema-children").is_some() {
                errors.push(format!(
                    "schema-nodes[{path:?}] is a leaf/leaf-list/anydata/anyxml but carries a \"schema-children\" field"
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
        let mut values = std::collections::HashSet::new();
        collect_identityref_values(&t["type"], &mut values);
        if !values.is_empty() {
            errors.push(format!(
                "typedefs[{key:?}]'s own type descriptor must never carry identityref values"
            ));
        }
    }

    let mut all_values = std::collections::HashSet::new();
    for node in schema_nodes.values() {
        if let Some(t) = node.get("type") {
            collect_identityref_values(t, &mut all_values);
        }
    }
    let listed_implemented: std::collections::HashSet<String> = modules
        .values()
        .flat_map(|m| m["identities"].as_array().unwrap())
        .map(|i| i.as_str().unwrap().to_string())
        .collect();
    for name in all_values.difference(&listed_implemented) {
        errors.push(format!(
            "{name:?} is a value of an identityref but no module lists it as implemented"
        ));
    }
    for name in listed_implemented.difference(&all_values) {
        errors.push(format!(
            "{name:?} is listed as implemented but is no identityref's value"
        ));
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "yang catalog has {} consistency error(s):\n{}",
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
            crate::schema::build(&[src.to_string()], &[sid_src.to_string()], &[], &[], &[])
                .unwrap();
        let doc = to_yang_catalog(&schema).unwrap();

        assert!(doc["typedefs"].get("foo:percent").is_some());
        assert!(doc["typedefs"].get("/foo:bar inner-percent").is_some());
        assert_eq!(
            doc["modules"]["foo"]["typedefs"],
            serde_json::json!(["foo:percent", "/foo:bar inner-percent"])
        );

        let schema_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("schema")
            .join("yang-catalog-schema-v1.0.0.json");
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
            &[],
        )
        .unwrap();
        let Json::Object(doc) = to_yang_catalog(&schema).unwrap() else {
            unreachable!()
        };
        doc
    }

    fn verify_error(doc: &Map<String, Json>) -> String {
        verify_yang_catalog(doc).expect_err("the corrupted document must fail verification")
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
        assert_eq!(
            doc["modules"]["foo"]["schema-root-nodes"],
            serde_json::json!([])
        );
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
        doc["modules"]["foo"]["schema-root-nodes"] = serde_json::json!(["/foo:error"]);
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

    fn deviation_doc() -> Map<String, Json> {
        let host = r#"
            module host {
              namespace "urn:host";
              prefix h;

              leaf a {
                type string;
              }
              leaf b {
                type string;
              }
            }
        "#;
        let zed_dev = r#"
            module zed-dev {
              namespace "urn:zed-dev";
              prefix z;

              import host {
                prefix h;
              }

              deviation "/h:a" {
                deviate not-supported;
              }
            }
        "#;
        let abc_dev = r#"
            module abc-dev {
              namespace "urn:abc-dev";
              prefix ab;

              import host {
                prefix h;
              }

              deviation "/h:b" {
                deviate not-supported;
              }
            }
        "#;
        let sid_src = r#"{
            "module-name": "host",
            "module-revision": "2026-01-01",
            "items": [
                {"namespace": "module", "identifier": "host", "sid": 1000},
                {"namespace": "module", "identifier": "zed-dev", "sid": 1100},
                {"namespace": "module", "identifier": "abc-dev", "sid": 1200},
                {"namespace": "data", "identifier": "/host:a", "sid": 1001},
                {"namespace": "data", "identifier": "/host:b", "sid": 1002}
            ]
        }"#;
        let schema = crate::schema::build(
            &[host.to_string(), zed_dev.to_string(), abc_dev.to_string()],
            &[sid_src.to_string()],
            &[],
            &[],
            &[],
        )
        .unwrap();
        let Json::Object(doc) = to_yang_catalog(&schema).unwrap() else {
            unreachable!()
        };
        doc
    }

    #[test]
    fn a_modules_deviations_list_every_deviating_module_sorted() {
        // zed-dev is declared before abc-dev, so this also checks the
        // list is sorted rather than in declaration order.
        let doc = deviation_doc();
        assert_eq!(
            doc["modules"]["host"]["deviations"],
            serde_json::json!(["abc-dev", "zed-dev"])
        );
        assert_eq!(
            doc["modules"]["abc-dev"]["deviations"],
            serde_json::json!([])
        );
        assert_eq!(
            doc["modules"]["zed-dev"]["deviations"],
            serde_json::json!([])
        );
    }

    #[test]
    fn verify_rejects_a_deviation_that_is_not_an_implemented_module() {
        let mut doc = deviation_doc();
        doc["modules"]["host"]["deviations"] = serde_json::json!(["abc-dev", "nowhere"]);
        let e = verify_error(&doc);
        assert!(
            e.contains("modules[\"host\"].deviations: \"nowhere\" not found in modules"),
            "{e}"
        );
    }

    #[test]
    fn verify_rejects_a_module_listed_as_deviating_itself() {
        let mut doc = deviation_doc();
        doc["modules"]["host"]["deviations"] = serde_json::json!(["abc-dev", "host", "zed-dev"]);
        let e = verify_error(&doc);
        assert!(
            e.contains("modules[\"host\"].deviations lists the module itself"),
            "{e}"
        );
    }

    #[test]
    fn verify_rejects_unsorted_or_duplicate_deviations() {
        let mut doc = deviation_doc();
        doc["modules"]["host"]["deviations"] = serde_json::json!(["zed-dev", "abc-dev"]);
        let e = verify_error(&doc);
        assert!(
            e.contains("is not sorted alphabetically without duplicates"),
            "{e}"
        );

        let mut doc = deviation_doc();
        doc["modules"]["host"]["deviations"] = serde_json::json!(["abc-dev", "abc-dev"]);
        let e = verify_error(&doc);
        assert!(
            e.contains("is not sorted alphabetically without duplicates"),
            "{e}"
        );
    }

    fn identity_doc() -> Map<String, Json> {
        let lib = r#"
            module lib {
              namespace "urn:lib";
              prefix l;

              identity animal;
            }
        "#;
        let foo = r#"
            module foo {
              namespace "urn:foo";
              prefix f;

              import lib {
                prefix l;
              }

              identity pet {
                base l:animal;
              }
              identity dog {
                base pet;
              }
              identity cat {
                base pet;
              }

              leaf kind {
                type identityref {
                  base l:animal;
                }
              }
            }
        "#;
        let sid_src = r#"{
            "module-name": "foo",
            "module-revision": "2026-01-01",
            "items": [
                {"namespace": "module", "identifier": "foo", "sid": 1000},
                {"namespace": "data", "identifier": "/foo:kind", "sid": 1001},
                {"namespace": "identity", "identifier": "pet", "sid": 1010},
                {"namespace": "identity", "identifier": "dog", "sid": 1011},
                {"namespace": "identity", "identifier": "cat", "sid": 1012}
            ]
        }"#;
        let lib_sid = r#"{
            "module-name": "lib",
            "module-revision": "2026-01-01",
            "items": [
                {"namespace": "module", "identifier": "lib", "sid": 2000},
                {"namespace": "identity", "identifier": "animal", "sid": 2010}
            ]
        }"#;
        let accepted: &[&str] = &["foo:dog"];
        let schema = crate::schema::build(
            &[lib.to_string(), foo.to_string()],
            &[sid_src.to_string(), lib_sid.to_string()],
            &[],
            &[("lib:animal", accepted)],
            &[],
        )
        .unwrap();
        let Json::Object(doc) = to_yang_catalog(&schema).unwrap() else {
            unreachable!()
        };
        doc
    }

    #[test]
    fn identities_split_into_implemented_values_and_bases_by_module_kind() {
        // "pet" is kept as dog's base but isn't accepted, "cat" isn't
        // accepted or a base of anything accepted, and "animal" is the
        // leaf's own base, declared in import-only "lib".
        let doc = identity_doc();
        let mut keys: Vec<&String> = doc["identities"].as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(keys, vec!["foo:dog", "foo:pet", "lib:animal"]);
        assert_eq!(
            doc["modules"]["foo"]["identities"],
            serde_json::json!(["foo:dog"])
        );
        assert_eq!(
            doc["import-only-modules"]["lib"]["identities"],
            serde_json::json!(["lib:animal"])
        );
    }

    #[test]
    fn verify_rejects_an_identity_neither_implemented_nor_referred_to_as_a_base() {
        let mut doc = identity_doc();
        doc["modules"]["foo"]["identities"] = serde_json::json!([]);
        let e = verify_error(&doc);
        assert!(
            e.contains("identities[\"foo:dog\"] is neither implemented nor referred to as a base by anything implemented"),
            "{e}"
        );
    }

    #[test]
    fn verify_rejects_an_import_only_identity_nothing_implemented_refers_to() {
        // A synthetic "lib:plant" listed by "lib" although nothing
        // implemented refers to it: it's neither a leaf's base nor
        // reachable from "dog" through base statements.
        let mut doc = identity_doc();
        let mut plant = doc["identities"]["lib:animal"].clone();
        plant["name"] = serde_json::json!("plant");
        plant["sid"] = serde_json::json!("2011");
        doc["identities"]
            .as_object_mut()
            .unwrap()
            .insert("lib:plant".to_string(), plant);
        doc["sid-index"].as_object_mut().unwrap().insert(
            "2011".to_string(),
            serde_json::json!({"namespace": "identity", "identifier": "lib:plant"}),
        );
        doc["import-only-modules"]["lib"]["identities"] =
            serde_json::json!(["lib:animal", "lib:plant"]);
        let e = verify_error(&doc);
        assert!(
            e.contains("import-only-modules[\"lib\"].identities lists \"lib:plant\", which nothing implemented refers to as a base"),
            "{e}"
        );
    }

    #[test]
    fn verify_rejects_an_identity_listed_by_two_modules() {
        let mut doc = identity_doc();
        doc["import-only-modules"]["lib"]["identities"] =
            serde_json::json!(["lib:animal", "foo:dog"]);
        let e = verify_error(&doc);
        assert!(
            e.contains("identities[\"foo:dog\"] is listed by more than one module"),
            "{e}"
        );
        assert!(
            e.contains("is listed by import-only-modules[\"lib\"], but is declared in \"foo\""),
            "{e}"
        );
    }

    #[test]
    fn verify_rejects_an_import_only_modules_identity_listed_by_no_module() {
        let mut doc = identity_doc();
        doc["import-only-modules"]["lib"]["identities"] = serde_json::json!([]);
        let e = verify_error(&doc);
        assert!(
            e.contains("identities[\"lib:animal\"] is listed by no module"),
            "{e}"
        );
    }

    #[test]
    fn a_leaf_bases_own_base_is_listed_as_a_base_only() {
        // "pet" is the leaf's base and has a base of its own, "animal",
        // which no leaf names. The implemented "dog" still refers to
        // "animal" through base statements, so it's listed, and the
        // verifier must accept that.
        let src = r#"
            module foo {
              namespace "urn:foo";
              prefix f;

              identity animal;
              identity pet {
                base animal;
              }
              identity dog {
                base pet;
              }

              leaf kind {
                type identityref {
                  base pet;
                }
              }
            }
        "#;
        let sid_src = r#"{
            "module-name": "foo",
            "module-revision": "2026-01-01",
            "items": [
                {"namespace": "module", "identifier": "foo", "sid": 1000},
                {"namespace": "data", "identifier": "/foo:kind", "sid": 1001},
                {"namespace": "identity", "identifier": "animal", "sid": 1010},
                {"namespace": "identity", "identifier": "pet", "sid": 1011},
                {"namespace": "identity", "identifier": "dog", "sid": 1012}
            ]
        }"#;
        let schema =
            crate::schema::build(&[src.to_string()], &[sid_src.to_string()], &[], &[], &[])
                .unwrap();
        let doc = to_yang_catalog(&schema).unwrap();
        let mut keys: Vec<&String> = doc["identities"].as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(keys, vec!["foo:animal", "foo:dog", "foo:pet"]);
        assert_eq!(
            doc["modules"]["foo"]["identities"],
            serde_json::json!(["foo:dog"])
        );
    }

    #[test]
    fn a_leaf_base_without_values_still_lists_its_own_base() {
        // Nothing derives from "pet", so the leaf has no valid values and
        // nothing is implemented, but the implemented leaf still refers
        // to "pet" as its base, and through it to "pet"'s own base
        // "animal".
        let src = r#"
            module foo {
              namespace "urn:foo";
              prefix f;

              identity animal;
              identity pet {
                base animal;
              }

              leaf kind {
                type identityref {
                  base pet;
                }
              }
            }
        "#;
        let sid_src = r#"{
            "module-name": "foo",
            "module-revision": "2026-01-01",
            "items": [
                {"namespace": "module", "identifier": "foo", "sid": 1000},
                {"namespace": "data", "identifier": "/foo:kind", "sid": 1001},
                {"namespace": "identity", "identifier": "animal", "sid": 1010},
                {"namespace": "identity", "identifier": "pet", "sid": 1011}
            ]
        }"#;
        let schema =
            crate::schema::build(&[src.to_string()], &[sid_src.to_string()], &[], &[], &[])
                .unwrap();
        let doc = to_yang_catalog(&schema).unwrap();
        let mut keys: Vec<&String> = doc["identities"].as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(keys, vec!["foo:animal", "foo:pet"]);
        assert_eq!(doc["modules"]["foo"]["identities"], serde_json::json!([]));
    }

    #[test]
    fn an_identityref_carries_its_own_sorted_values() {
        let doc = identity_doc();
        assert_eq!(
            doc["schema-nodes"]["/foo:kind"]["type"]["values"],
            serde_json::json!(["foo:dog"])
        );
    }

    #[test]
    fn verify_rejects_an_identityref_without_values() {
        let mut doc = identity_doc();
        doc["schema-nodes"]["/foo:kind"]["type"]
            .as_object_mut()
            .unwrap()
            .remove("values");
        let e = verify_error(&doc);
        assert!(e.contains("an identityref has no \"values\""), "{e}");
    }

    #[test]
    fn verify_rejects_an_identityref_value_not_derived_from_its_base() {
        let mut doc = identity_doc();
        doc["schema-nodes"]["/foo:kind"]["type"]["values"] = serde_json::json!(["lib:animal"]);
        let e = verify_error(&doc);
        assert!(
            e.contains("\"lib:animal\" is not derived from every base"),
            "{e}"
        );
    }

    #[test]
    fn verify_rejects_an_identityref_value_missing_from_identities() {
        let mut doc = identity_doc();
        doc["schema-nodes"]["/foo:kind"]["type"]["values"] =
            serde_json::json!(["foo:dog", "foo:wolf"]);
        let e = verify_error(&doc);
        assert!(e.contains("\"foo:wolf\" not found in identities"), "{e}");
    }

    #[test]
    fn verify_rejects_unsorted_identityref_values() {
        let mut doc = identity_doc();
        doc["schema-nodes"]["/foo:kind"]["type"]["values"] =
            serde_json::json!(["foo:pet", "foo:dog"]);
        let e = verify_error(&doc);
        assert!(
            e.contains("values is not sorted alphabetically without duplicates"),
            "{e}"
        );
    }

    #[test]
    fn verify_rejects_a_value_its_module_does_not_list_as_implemented() {
        let mut doc = identity_doc();
        doc["schema-nodes"]["/foo:kind"]["type"]["values"] =
            serde_json::json!(["foo:dog", "foo:pet"]);
        let e = verify_error(&doc);
        assert!(
            e.contains(
                "\"foo:pet\" is a value of an identityref but no module lists it as implemented"
            ),
            "{e}"
        );
    }

    #[test]
    fn verify_rejects_an_implemented_identity_that_is_no_identityrefs_value() {
        let mut doc = identity_doc();
        doc["schema-nodes"]["/foo:kind"]["type"]["values"] = serde_json::json!([]);
        let e = verify_error(&doc);
        assert!(
            e.contains("\"foo:dog\" is listed as implemented but is no identityref's value"),
            "{e}"
        );
    }

    #[test]
    fn verify_rejects_values_on_a_typedefs_own_descriptor() {
        let src = r#"
            module foo {
              namespace "urn:foo";
              prefix f;

              identity animal;
              identity dog {
                base animal;
              }

              typedef animal-ref {
                type identityref {
                  base animal;
                }
              }

              leaf kind {
                type animal-ref;
              }
            }
        "#;
        let sid_src = r#"{
            "module-name": "foo",
            "module-revision": "2026-01-01",
            "items": [
                {"namespace": "module", "identifier": "foo", "sid": 1000},
                {"namespace": "data", "identifier": "/foo:kind", "sid": 1001},
                {"namespace": "identity", "identifier": "animal", "sid": 1010},
                {"namespace": "identity", "identifier": "dog", "sid": 1011}
            ]
        }"#;
        let schema =
            crate::schema::build(&[src.to_string()], &[sid_src.to_string()], &[], &[], &[])
                .unwrap();
        let Json::Object(mut doc) = to_yang_catalog(&schema).unwrap() else {
            unreachable!()
        };
        assert!(doc["typedefs"]["foo:animal-ref"]["type"]
            .get("values")
            .is_none());
        doc["typedefs"]["foo:animal-ref"]["type"]["values"] = serde_json::json!(["foo:dog"]);
        let e = verify_error(&doc);
        assert!(
            e.contains("typedefs[\"foo:animal-ref\"]'s own type descriptor must never carry identityref values"),
            "{e}"
        );
    }

    #[test]
    fn defaults_units_require_instance_and_invert_match_reach_the_document() {
        let src = r#"
            module foo {
              namespace "urn:foo";
              prefix f;

              typedef seconds {
                type uint8;
                default 5;
                units "seconds";
              }

              leaf timeout {
                type seconds;
              }
              leaf-list retries {
                type seconds;
              }
              leaf target {
                type string {
                  pattern "x.*" {
                    modifier invert-match;
                  }
                }
              }
              leaf ref {
                type leafref {
                  path "/f:target";
                  require-instance false;
                }
              }
              leaf node {
                type instance-identifier;
              }
            }
        "#;
        let sid_src = r#"{
            "module-name": "foo",
            "module-revision": "2026-01-01",
            "items": [
                {"namespace": "module", "identifier": "foo", "sid": 1000},
                {"namespace": "data", "identifier": "/foo:timeout", "sid": 1001},
                {"namespace": "data", "identifier": "/foo:retries", "sid": 1002},
                {"namespace": "data", "identifier": "/foo:target", "sid": 1003},
                {"namespace": "data", "identifier": "/foo:ref", "sid": 1004},
                {"namespace": "data", "identifier": "/foo:node", "sid": 1005}
            ]
        }"#;
        let schema =
            crate::schema::build(&[src.to_string()], &[sid_src.to_string()], &[], &[], &[])
                .unwrap();
        let doc = to_yang_catalog(&schema).unwrap();
        let typedef = &doc["typedefs"]["foo:seconds"];
        assert_eq!(typedef["default"], "5");
        assert_eq!(typedef["units"], "seconds");
        let nodes = &doc["schema-nodes"];
        assert_eq!(nodes["/foo:timeout"]["default"], "5");
        assert_eq!(nodes["/foo:timeout"]["units"], "seconds");
        assert_eq!(nodes["/foo:retries"]["default"], serde_json::json!(["5"]));
        assert_eq!(nodes["/foo:retries"]["units"], "seconds");
        assert_eq!(
            nodes["/foo:target"]["type"]["pattern"],
            serde_json::json!([{"pattern": "x.*", "invert-match": true}])
        );
        assert_eq!(nodes["/foo:ref"]["type"]["require-instance"], false);
        assert_eq!(nodes["/foo:node"]["type"]["require-instance"], true);
    }

    #[test]
    fn range_length_and_pattern_error_info_reach_the_document() {
        let src = r#"
            module foo {
              namespace "urn:foo";
              prefix f;

              leaf level {
                type uint8 {
                  range "0..100" {
                    error-message "must be a percentage";
                  }
                }
              }
              leaf big {
                type uint64 {
                  range "1..max" {
                    error-app-tag "too-small";
                  }
                }
              }
              leaf name {
                type string {
                  length "1..8" {
                    error-message "1 to 8 characters";
                  }
                  pattern "[a-z]+" {
                    error-message "lower case only";
                    error-app-tag "bad-case";
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
                {"namespace": "data", "identifier": "/foo:level", "sid": 1001},
                {"namespace": "data", "identifier": "/foo:big", "sid": 1002},
                {"namespace": "data", "identifier": "/foo:name", "sid": 1003}
            ]
        }"#;
        let schema =
            crate::schema::build(&[src.to_string()], &[sid_src.to_string()], &[], &[], &[])
                .unwrap();
        let doc = to_yang_catalog(&schema).unwrap();
        let nodes = &doc["schema-nodes"];
        assert_eq!(
            nodes["/foo:level"]["type"]["range"],
            serde_json::json!({"bounds": [[0, 100]], "error-message": "must be a percentage"})
        );
        assert_eq!(
            nodes["/foo:big"]["type"]["range"]["error-app-tag"],
            "too-small"
        );
        assert_eq!(
            nodes["/foo:name"]["type"]["length"]["error-message"],
            "1 to 8 characters"
        );
        assert_eq!(
            nodes["/foo:name"]["type"]["pattern"],
            serde_json::json!([{
                "pattern": "[a-z]+",
                "invert-match": false,
                "error-message": "lower case only",
                "error-app-tag": "bad-case"
            }])
        );
    }

    #[test]
    fn verify_rejects_children_on_a_node_that_cannot_have_any() {
        let mut doc = identity_doc();
        doc["schema-nodes"]["/foo:kind"]["schema-children"] = serde_json::json!([]);
        let e = verify_error(&doc);
        assert!(
            e.contains("schema-nodes[\"/foo:kind\"] is a leaf/leaf-list/anydata/anyxml but carries a \"schema-children\" field"),
            "{e}"
        );

        let mut doc = identity_doc();
        doc["schema-nodes"]["/foo:kind"]["data-children"] = serde_json::json!([]);
        let e = verify_error(&doc);
        assert!(
            e.contains("schema-nodes[\"/foo:kind\"] is a leaf/leaf-list/anydata/anyxml but carries a \"data-children\" field"),
            "{e}"
        );
    }
}
