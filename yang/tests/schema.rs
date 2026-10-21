// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

use yang::schema::*;

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
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    let bar = schema.node_by_data_path("/foo:bar").unwrap();
    assert_eq!(bar.status, Some(Status::Current));
    let baz = bar.find_child("baz").unwrap();
    assert_eq!(baz.status, Some(Status::Current));
}

#[test]
fn a_yang_data_roots_status_defaults_to_current_too() {
    // The root is created outside interpret_schema_node's own
    // default-status handling (see interpret_extension), so it needs
    // its own path to the same RFC 7950 7.21.2 default. The template
    // is accept-listed, since only an accept-listed yang-data template
    // is exposed at all -- see
    // a_yang_data_template_not_on_the_accept_list_is_absent_entirely.
    let src = r#"
        module ietf-coreconf {
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
    let schema = build(
        &[src.to_string()],
        &[],
        &["ietf-coreconf:error-template"],
        &[],
        &[],
    )
    .unwrap();
    let error_root = schema
        .yang_data()
        .find(|y| y.name == "error-template")
        .unwrap()
        .root();
    assert_eq!(error_root.status, Some(Status::Current));
}

#[test]
fn a_yang_data_roots_config_is_always_false_even_with_an_explicit_config_true() {
    // RFC 8040 Sec. 8: "The config-stmt is ignored if present" inside
    // a yang-data extension's content -- an explicit `config true`
    // must not survive into the resulting node, unlike an ordinary
    // schema node where it would. The template is accept-listed, same
    // reason as a_yang_data_roots_status_defaults_to_current_too above.
    let src = r#"
        module ietf-coreconf {
          namespace "urn:foo";
          prefix f;

          rc:yang-data error-template {
            container error {
              config true;
              leaf message {
                config true;
                type string;
              }
            }
          }
        }
    "#;
    let schema = build(
        &[src.to_string()],
        &[],
        &["ietf-coreconf:error-template"],
        &[],
        &[],
    )
    .unwrap();
    let error_root = schema
        .yang_data()
        .find(|y| y.name == "error-template")
        .unwrap()
        .root();
    assert!(!error_root.config);
    let message = error_root.find_child("message").unwrap();
    assert!(!message.config);
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
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
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
    let err = build(&[src.to_string()], &[], &[], &[], &[]).err().unwrap();
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
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
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
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    let input = schema.node_by_schema_path("/foo:bar/input").unwrap();
    let output = schema.node_by_schema_path("/foo:bar/output").unwrap();
    assert_eq!(input.status, None);
    assert_eq!(output.status, None);
}

#[test]
fn synthetic_marks_only_nodes_absent_from_the_source_text() {
    // rpc bar's `input` and `output` are absent from the source, so
    // fercc fabricates them (RFC 7950 7.14 says they always exist in
    // the schema tree); `rpc baz`'s are explicit. `choice pick`'s
    // `leaf sub` has no `case` wrapper (RFC 7950 7.9.2 shorthand), so
    // fercc synthesizes one; `choice other`'s `case explicit` is
    // written out.
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          rpc bar;

          rpc baz {
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

          container top {
            choice pick {
              leaf sub {
                type string;
              }
            }
            choice other {
              case explicit {
                leaf sub2 {
                  type string;
                }
              }
            }
          }
        }
    "#;
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();

    let bar_input = schema.node_by_schema_path("/foo:bar/input").unwrap();
    let bar_output = schema.node_by_schema_path("/foo:bar/output").unwrap();
    assert!(bar_input.synthetic);
    assert!(bar_output.synthetic);

    let baz_input = schema.node_by_schema_path("/foo:baz/input").unwrap();
    let baz_output = schema.node_by_schema_path("/foo:baz/output").unwrap();
    assert!(!baz_input.synthetic);
    assert!(!baz_output.synthetic);

    let implicit_case = schema.node_by_schema_path("/foo:top/pick/sub").unwrap();
    assert!(implicit_case.synthetic);

    let explicit_case = schema
        .node_by_schema_path("/foo:top/other/explicit")
        .unwrap();
    assert!(!explicit_case.synthetic);

    let top = schema.node_by_schema_path("/foo:top").unwrap();
    assert!(!top.synthetic);
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
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
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
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
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

          leaf pet {
            type identityref {
              base animal;
            }
          }
        }
    "#;
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    let animal = schema.identity_by_name("foo", "animal").unwrap();
    assert_eq!(animal.name, "animal");
    let dog = schema.identity_by_name("foo", "dog").unwrap();
    assert_eq!(dog.base().count(), 1);
    assert_eq!(dog.base().next().unwrap().name, "animal");
    assert!(schema.identity_by_name("foo", "no-such-identity").is_none());
}

#[test]
fn identities_only_lists_ones_reachable_from_an_identityref_usage() {
    // schema.identities() must reflect only identities actually
    // reachable as a legal identityref value from some real node --
    // an identity declared in a module's text but never named as a
    // base of any identityref, directly or via derivation, must not
    // appear. "dog" is reachable only transitively (nothing uses
    // identityref base dog directly -- it's derived from animal,
    // which the real usage names), confirming the derived-identity
    // closure is included too. identity_by_name applies the same
    // filter (it's a thin wrapper around identities()), so it can't
    // be used to bypass this.
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          identity animal;
          identity dog {
            base animal;
          }
          identity unused;

          leaf pet {
            type identityref {
              base animal;
            }
          }
        }
    "#;
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    let mut names: Vec<String> = schema.identities().map(|i| i.name.clone()).collect();
    names.sort();
    assert_eq!(names, vec!["animal", "dog"]);
    assert!(schema.identity_by_name("foo", "unused").is_none());
}

#[test]
fn an_identitys_description_is_read_from_its_own_statement() {
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          identity animal {
            description "a living creature";
          }
          identity rock;

          leaf pet {
            type identityref {
              base animal;
            }
          }
          leaf collectible {
            type identityref {
              base rock;
            }
          }
        }
    "#;
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    let animal = schema.identity_by_name("foo", "animal").unwrap();
    assert_eq!(animal.description.as_deref(), Some("a living creature"));
    let rock = schema.identity_by_name("foo", "rock").unwrap();
    assert_eq!(rock.description, None);
}

#[test]
fn module_by_sid_finds_a_module_with_an_attached_sid() {
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          leaf bar {
            type string;
          }
        }
    "#;
    let sid = r#"{
        "module-name": "foo",
        "module-revision": "2026-01-01",
        "items": [
            {"namespace": "module", "identifier": "foo", "sid": 5000}
        ]
    }"#;
    let schema = build(&[src.to_string()], &[sid.to_string()], &[], &[], &[]).unwrap();
    let module = schema.module_by_sid(5000).unwrap();
    assert_eq!(module.name, "foo");
    assert!(schema.module_by_sid(9999).is_none());
}

#[test]
fn a_sid_files_data_identifier_that_resolves_to_no_node_is_a_schema_error() {
    // SIDs are attached before deviations are applied, so every data
    // identifier a .sid file names must resolve against the
    // pre-deviation schema tree. A miss means the .sid file and the
    // YANG text disagree, not that the node was deviated away.
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          container top {
            leaf a {
              type string;
            }
          }
        }
    "#;
    let sid = r#"{
        "module-name": "foo",
        "module-revision": "2026-01-01",
        "items": [
            {"namespace": "data", "identifier": "/foo:top", "sid": 1001},
            {"namespace": "data", "identifier": "/foo:top/missing", "sid": 1002}
        ]
    }"#;
    let err = build(&[src.to_string()], &[sid.to_string()], &[], &[], &[])
        .err()
        .expect("an unresolvable .sid data identifier must fail");
    assert!(err.0.contains("/foo:top/missing"), "{}", err.0);
}

#[test]
fn a_sid_files_data_identifier_for_a_since_deviated_away_node_is_not_an_error() {
    // The .sid file is generated from the module itself, so it names
    // the node a device's own deviation module later removes. That is
    // consistent, not a .sid file bug: the SID attaches to the
    // pre-deviation node, which the deviation then removes.
    let foo = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          container top {
            leaf keep {
              type string;
            }
            leaf drop {
              type string;
            }
          }
        }
    "#;
    let dev = r#"
        module foo-dev {
          namespace "urn:foo-dev";
          prefix fd;

          import foo {
            prefix f;
          }

          deviation "/f:top/f:drop" {
            deviate not-supported;
          }
        }
    "#;
    let sid = r#"{
        "module-name": "foo",
        "module-revision": "2026-01-01",
        "items": [
            {"namespace": "data", "identifier": "/foo:top", "sid": 1001},
            {"namespace": "data", "identifier": "/foo:top/keep", "sid": 1002},
            {"namespace": "data", "identifier": "/foo:top/drop", "sid": 1003}
        ]
    }"#;
    let schema = build(
        &[foo.to_string(), dev.to_string()],
        &[sid.to_string()],
        &[],
        &[],
        &[],
    )
    .unwrap();
    let top = schema.node_by_schema_path("/foo:top").unwrap();
    assert!(top.find_child("drop").is_none());
    assert_eq!(top.find_child("keep").unwrap().sid, Some(1002));
    assert!(schema.node_by_sid(1003).is_none());
}

#[test]
fn a_sid_files_data_identifier_can_attach_a_sid_to_a_yang_data_root() {
    // Real .sid files address a yang-data root's own container
    // directly by its full qualified name (e.g.
    // "/ietf-coreconf:error") -- confirmed against a real catalog's
    // ietf-coreconf@*.sid. Attaching that SID must reach the
    // yang-data root, not just ordinary module schema nodes. The
    // template is accept-listed, same reason as
    // a_yang_data_roots_status_defaults_to_current_too above.
    let src = r#"
        module ietf-coreconf {
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
    let sid = r#"{
        "module-name": "ietf-coreconf",
        "module-revision": "2026-01-01",
        "items": [
            {"namespace": "data", "identifier": "/ietf-coreconf:error", "sid": 38455},
            {"namespace": "data", "identifier": "/ietf-coreconf:error/message", "sid": 38456}
        ]
    }"#;
    let schema = build(
        &[src.to_string()],
        &[sid.to_string()],
        &["ietf-coreconf:coreconf-error"],
        &[],
        &[],
    )
    .unwrap();
    let error_root = schema
        .yang_data()
        .find(|y| y.name == "coreconf-error")
        .unwrap()
        .root();
    assert_eq!(error_root.sid, Some(38455));
    let message = error_root.find_child("message").unwrap();
    assert_eq!(message.sid, Some(38456));
}

#[test]
fn two_modules_augmenting_the_same_target_with_identically_named_children_resolve_independently() {
    // Both bar and baz augment foo:top with their own container
    // named "thing" -- a schema-node-id/.sid identifier naming one
    // must never resolve to the other, even though their bare local
    // names collide.
    let foo = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          container top;
        }
    "#;
    let bar = r#"
        module bar {
          namespace "urn:bar";
          prefix b;
          import foo { prefix f; }

          augment "/f:top" {
            container thing {
              leaf x {
                type string;
              }
            }
          }
        }
    "#;
    let baz = r#"
        module baz {
          namespace "urn:baz";
          prefix z;
          import foo { prefix f; }

          augment "/f:top" {
            container thing {
              leaf y {
                type string;
              }
            }
          }
        }
    "#;
    let sid = r#"{
        "module-name": "bar",
        "module-revision": "2026-01-01",
        "items": [
            {"namespace": "data", "identifier": "/foo:top/bar:thing", "sid": 7001},
            {"namespace": "data", "identifier": "/foo:top/bar:thing/x", "sid": 7002}
        ]
    }"#;
    let sid2 = r#"{
        "module-name": "baz",
        "module-revision": "2026-01-01",
        "items": [
            {"namespace": "data", "identifier": "/foo:top/baz:thing", "sid": 8001},
            {"namespace": "data", "identifier": "/foo:top/baz:thing/y", "sid": 8002}
        ]
    }"#;
    let schema = build(
        &[foo.to_string(), bar.to_string(), baz.to_string()],
        &[sid.to_string(), sid2.to_string()],
        &[],
        &[],
        &[],
    )
    .unwrap();

    let bar_thing = schema.node_by_schema_path("/foo:top/bar:thing").unwrap();
    assert_eq!(bar_thing.sid, Some(7001));
    assert!(bar_thing.find_child("x").is_some());
    assert_eq!(bar_thing.find_child("x").unwrap().sid, Some(7002));

    let baz_thing = schema.node_by_schema_path("/foo:top/baz:thing").unwrap();
    assert_eq!(baz_thing.sid, Some(8001));
    assert!(baz_thing.find_child("y").is_some());
    assert_eq!(baz_thing.find_child("y").unwrap().sid, Some(8002));
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
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
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
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
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
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
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
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    let bar = schema.node_by_data_path("/foo:bar").unwrap();

    let inline = bar.find_child("inline").unwrap().ty().unwrap();
    assert!(inline.base().is_none());

    let named = bar.find_child("named").unwrap().ty().unwrap();
    let base = named.base().unwrap();
    assert_eq!(base.name, "percent");
    assert_eq!(base.scope, "foo");
}

#[test]
fn a_leaf_using_a_typedef_derived_from_another_typedef_has_both_base_hops() {
    // typedef narrow derived-from typedef wide, and a leaf uses
    // narrow -- the leaf's own base must point at narrow (one hop),
    // and narrow's own canonical entry's base must point at wide
    // (the next hop), not either flattened into the other.
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          typedef wide {
            type uint8 {
              range "0..100";
            }
          }

          typedef narrow {
            type wide {
              range "0..50";
            }
          }

          leaf bar {
            type narrow;
          }
        }
    "#;
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();

    let bar = schema.node_by_data_path("/foo:bar").unwrap();
    let bar_ty = bar.ty().unwrap();
    let bar_base = bar_ty.base().unwrap();
    assert_eq!(bar_base.name, "narrow");
    assert_eq!(bar_ty.ranges, vec![(0, 50)]);

    let narrow = schema.typedef_by_name("foo", "narrow").unwrap();
    assert_eq!(narrow.ty().ranges, vec![(0, 50)]);
    let narrow_base = narrow.ty().base().unwrap();
    assert_eq!(narrow_base.name, "wide");

    let wide = schema.typedef_by_name("foo", "wide").unwrap();
    assert_eq!(wide.ty().ranges, vec![(0, 100)]);
    assert!(wide.ty().base().is_none());
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
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    let names: Vec<String> = schema.typedefs().map(|t| t.name.clone()).collect();
    assert_eq!(names, vec!["percent"]);
    let percent = schema.typedef_by_name("foo", "percent").unwrap();
    assert_eq!(
        percent.description.as_deref(),
        Some("a percentage, 0 to 100")
    );
}

#[test]
fn typedefs_are_listed_in_true_source_declaration_order_not_first_use_order() {
    // "z" is declared before "a" at module level, but "a" is
    // referenced first (by "early") -- first-use order would list
    // "a" before "z"; true source declaration order (what
    // interpret_scope_typedefs now guarantees) lists "z" before "a"
    // regardless.
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          typedef z {
            type uint8;
          }
          typedef a {
            type uint8;
          }

          leaf early {
            type a;
          }
          leaf late {
            type z;
          }
        }
    "#;
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    let names: Vec<String> = schema.typedefs().map(|t| t.name.clone()).collect();
    assert_eq!(names, vec!["z", "a"]);
}

#[test]
fn typedefs_nested_in_a_container_keep_their_own_declaration_order() {
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          container bar {
            typedef z {
              type uint8;
            }
            typedef a {
              type uint8;
            }

            leaf early {
              type a;
            }
            leaf late {
              type z;
            }
          }
        }
    "#;
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    let names: Vec<String> = schema.typedefs().map(|t| t.name.clone()).collect();
    assert_eq!(names, vec!["z", "a"]);
}

#[test]
fn typedefs_declared_in_a_grouping_keep_source_order_independently_per_use_site() {
    // Mirrors a_typedef_declared_directly_in_a_grouping_gets_an_
    // independent_identity_per_use_site's independence, but checks
    // the ORDER each use site's own pair ends up in: both "bar"'s
    // and "baz"'s own two typedefs (from the same grouping "g",
    // declared z-then-a) must independently come out z-then-a, not
    // reordered by which leaf references which first ("bar" uses
    // "a" before "z"; "baz" uses "z" before "a").
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          grouping g {
            typedef z {
              type uint8;
            }
            typedef a {
              type uint8;
            }
          }

          container bar {
            uses g;
            leaf early {
              type a;
            }
            leaf late {
              type z;
            }
          }

          container baz {
            uses g;
            leaf early {
              type z;
            }
            leaf late {
              type a;
            }
          }
        }
    "#;
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    let names_and_scopes: Vec<(String, String)> = schema
        .typedefs()
        .map(|t| (t.name.clone(), t.scope.clone()))
        .collect();
    assert_eq!(
        names_and_scopes,
        vec![
            ("z".to_string(), "/foo:bar".to_string()),
            ("a".to_string(), "/foo:bar".to_string()),
            ("z".to_string(), "/foo:baz".to_string()),
            ("a".to_string(), "/foo:baz".to_string()),
        ]
    );
}

#[test]
fn a_plain_unreferenced_typedef_is_unreachable() {
    // A typedef is predeclared (and its own type eagerly
    // interpreted) regardless of whether anything ever references
    // it -- see a_malformed_unreferenced_typedef_is_a_schema_error
    // for why that matters -- but it must still never appear in
    // typedefs()/typedef_by_name unless something reachable
    // actually uses it, exactly like
    // typedefs_excludes_one_whose_only_use_site_is_deviated_away's
    // reachability filtering.
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          typedef percent {
            type uint8 {
              range "0..100";
            }
          }

          leaf bar {
            type string;
          }
        }
    "#;
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    assert_eq!(schema.typedefs().count(), 0);
    assert!(schema.typedef_by_name("foo", "percent").is_none());
}

#[test]
fn a_malformed_unreferenced_typedef_is_a_schema_error() {
    // "unreachable" is a property of the document (which named
    // types/identities a reader can look up), not an excuse to
    // skip semantically checking a typedef's own well-formedness --
    // "bad" names a type that's neither a builtin nor another
    // typedef, so interpreting it must fail even though nothing
    // ever references "bad" itself.
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          typedef bad {
            type no-such-type;
          }

          leaf bar {
            type string;
          }
        }
    "#;
    let err = build(&[src.to_string()], &[], &[], &[], &[]).err().unwrap();
    assert!(err.0.contains("no-such-type"), "{}", err.0);
}

#[test]
fn a_plain_unreferenced_identity_is_unreachable() {
    // Unlike identities_only_lists_ones_reachable_from_an_identityref_
    // usage's "unused" case (declared but checked alongside others
    // that ARE reachable), this is the minimal case on its own: an
    // identity with zero identityref usage anywhere in the module.
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          identity animal;

          leaf bar {
            type string;
          }
        }
    "#;
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    assert_eq!(schema.identities().count(), 0);
    assert!(schema.identity_by_name("foo", "animal").is_none());
}

#[test]
fn typedefs_excludes_one_whose_only_use_site_is_deviated_away() {
    // schema.typedefs() must reflect only typedefs actually
    // reachable from this device's resolved tree -- a typedef
    // created as a side effect of building the schema (before
    // deviations run) but left with no surviving use site must not
    // appear. typedef_by_name applies the same filter (it's a thin
    // wrapper around typedefs()), so it can't be used to bypass this.
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          typedef percent {
            type uint8 {
              range "0..100";
            }
          }

          leaf gone {
            type percent;
          }

          deviation /f:gone {
            deviate not-supported;
          }
        }
    "#;
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    assert_eq!(schema.typedefs().count(), 0);
    assert!(schema.typedef_by_name("foo", "percent").is_none());
}

#[test]
fn typedefs_and_identities_exclude_ones_severed_by_a_deviate_replace_type() {
    // The node survives (unlike deviate not-supported) but its own
    // type substatement is replaced wholesale -- a different way to
    // sever reachability than removing the node outright. A typedef
    // or identity whose only real use site had its type replaced
    // away must stop being reachable too.
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          typedef percent {
            type uint8 {
              range "0..100";
            }
          }
          identity animal;

          leaf retyped {
            type percent;
          }
          leaf reidentified {
            type identityref {
              base animal;
            }
          }

          deviation /f:retyped {
            deviate replace {
              type uint8;
            }
          }
          deviation /f:reidentified {
            deviate replace {
              type string;
            }
          }
        }
    "#;
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    assert_eq!(schema.typedefs().count(), 0);
    assert!(schema.typedef_by_name("foo", "percent").is_none());
    assert_eq!(schema.identities().count(), 0);
    assert!(schema.identity_by_name("foo", "animal").is_none());

    let retyped = schema.node_by_data_path("/foo:retyped").unwrap();
    assert_eq!(retyped.ty().unwrap().builtin, Builtin::Uint8);
    let reidentified = schema.node_by_data_path("/foo:reidentified").unwrap();
    assert_eq!(reidentified.ty().unwrap().builtin, Builtin::String);
}

#[test]
fn a_typedef_nested_in_a_container_resolves_from_within_it() {
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          container bar {
            typedef percent {
              type uint8 {
                range "0..100";
              }
            }
            leaf x {
              type percent;
            }
          }
        }
    "#;
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    let x = schema.node_by_data_path("/foo:bar/x").unwrap();
    let ty = x.ty().unwrap();
    assert_eq!(ty.ranges, vec![(0, 100)]);
    let base = ty.base().unwrap();
    assert_eq!(base.name, "percent");
    assert_eq!(base.scope, "/foo:bar");

    // The reported scope must round-trip through the public lookup
    // entry point -- it's the external interface for addressing a
    // typedef by identity, not just a label to compare by eye.
    let looked_up = schema.typedef_by_name(&base.scope, "percent").unwrap();
    assert_eq!(looked_up.ty().ranges, vec![(0, 100)]);
    assert!(schema.typedef_by_name("foo", "percent").is_none());
}

#[test]
fn a_typedef_nested_in_a_container_is_invisible_outside_it() {
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          container bar {
            typedef percent {
              type uint8 {
                range "0..100";
              }
            }
            leaf x {
              type percent;
            }
          }

          container other {
            leaf y {
              type percent;
            }
          }
        }
    "#;
    let err = build(&[src.to_string()], &[], &[], &[], &[]).err().unwrap();
    assert!(err.0.contains("percent"), "{}", err.0);
}

#[test]
fn a_typedef_nested_in_a_list_rpc_input_and_notification_all_resolve() {
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          list entry {
            key "name";
            typedef percent {
              type uint8 {
                range "0..100";
              }
            }
            leaf name {
              type string;
            }
            leaf x {
              type percent;
            }
          }

          rpc do-it {
            input {
              typedef percent {
                type uint8 {
                  range "0..100";
                }
              }
              leaf x {
                type percent;
              }
            }
          }

          notification happened {
            typedef percent {
              type uint8 {
                range "0..100";
              }
            }
            leaf x {
              type percent;
            }
          }
        }
    "#;
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();

    let list_x = schema.node_by_data_path("/foo:entry/x").unwrap();
    assert_eq!(list_x.ty().unwrap().ranges, vec![(0, 100)]);

    let input_x = schema.node_by_schema_path("/foo:do-it/input/x").unwrap();
    assert_eq!(input_x.ty().unwrap().ranges, vec![(0, 100)]);

    let notif_x = schema.node_by_data_path("/foo:happened/x").unwrap();
    assert_eq!(notif_x.ty().unwrap().ranges, vec![(0, 100)]);
}

#[test]
fn a_grouping_nested_in_a_container_resolves_from_a_deeper_use_site() {
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          container bar {
            grouping g {
              leaf x {
                type string;
              }
            }

            container inner {
              uses g;
            }
          }
        }
    "#;
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    assert!(schema.node_by_data_path("/foo:bar/inner/x").is_some());
}

#[test]
fn a_grouping_nested_in_a_container_is_invisible_outside_it() {
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          container bar {
            grouping g {
              leaf x {
                type string;
              }
            }
          }

          container other {
            uses g;
          }
        }
    "#;
    let err = build(&[src.to_string()], &[], &[], &[], &[]).err().unwrap();
    assert!(err.0.contains("g"), "{}", err.0);
}

#[test]
fn a_grouping_nested_directly_in_another_grouping_resolves_when_spliced() {
    // RFC 7950 7.12.1 lets a `grouping` directly contain another
    // `grouping`. `outer`'s own `uses inner;` must resolve `inner`
    // against outer's own lexical position (RFC 7950 7.12:
    // "resolved relative to the scope in which the grouping is
    // defined, not where it is used") -- which only matters once
    // `outer` itself gets spliced somewhere else entirely.
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          grouping outer {
            grouping inner {
              leaf x {
                type string;
              }
            }
            uses inner;
          }

          container bar {
            uses outer;
          }
        }
    "#;
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    assert!(schema.node_by_data_path("/foo:bar/x").is_some());
}

#[test]
fn a_typedef_declared_directly_in_a_grouping_gets_an_independent_identity_per_use_site() {
    // RFC 7950 5.5: "The identifiers defined in the grouping are not
    // bound to a namespace until the contents of the grouping are
    // added to the schema tree via a 'uses' statement... at which
    // point they are bound to the namespace of the current module."
    // Two uses of the same grouping must produce two independent
    // typedefs, scoped to each use site, not one shared entry.
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          grouping g {
            typedef percent {
              type uint8 {
                range "0..100";
              }
            }
            leaf x {
              type percent;
            }
          }

          container bar {
            uses g;
          }

          container baz {
            uses g;
          }
        }
    "#;
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();

    let bar_x = schema.node_by_data_path("/foo:bar/x").unwrap();
    let bar_base = bar_x.ty().unwrap().base().unwrap();
    assert_eq!(bar_base.scope, "/foo:bar");

    let baz_x = schema.node_by_data_path("/foo:baz/x").unwrap();
    let baz_base = baz_x.ty().unwrap().base().unwrap();
    assert_eq!(baz_base.scope, "/foo:baz");

    // Two independent typedefs named "percent" now exist, one per
    // use site -- not a single shared entry.
    let names_and_scopes: Vec<(String, String)> = schema
        .typedefs()
        .map(|t| (t.name.clone(), t.scope.clone()))
        .collect();
    assert_eq!(
        names_and_scopes,
        vec![
            ("percent".to_string(), "/foo:bar".to_string()),
            ("percent".to_string(), "/foo:baz".to_string()),
        ]
    );

    // Each reported scope must look up its OWN independent entry
    // through the public interface -- not the other site's, and not
    // a single shared one.
    let via_bar = schema.typedef_by_name("/foo:bar", "percent").unwrap();
    let via_baz = schema.typedef_by_name("/foo:baz", "percent").unwrap();
    assert_eq!(via_bar.scope, "/foo:bar");
    assert_eq!(via_baz.scope, "/foo:baz");
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
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    let pick = schema.node_by_data_path("/foo:pick").unwrap();
    let target = pick.ty().unwrap().leafref_target().unwrap();
    assert_eq!(target.kw, "leaf");
    assert_eq!(target.local_name(), "name");
}

#[test]
fn a_typedefs_own_canonical_entry_never_gets_a_leafref_target() {
    // A typedef has no location in the schema tree, so there's no
    // single node a relative leafref path could even be resolved
    // against -- and even an absolute path's target, while
    // resolvable in principle, is never actually resolved onto the
    // typedef's own canonical descriptor. Only an actual use site's
    // own clone (`pick`'s, covered by
    // leafref_resolves_through_a_typedef_to_its_concrete_target)
    // ever gets a resolved target -- RFC 7950 9.9.2 defines the
    // leafref's context node as "the leaf or leaf-list node in the
    // data tree that references the typedef", never the typedef
    // itself.
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
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    let bar_ref = schema.typedef_by_name("foo", "bar-ref").unwrap();
    assert!(bar_ref.ty().leafref_target().is_none());
}

#[test]
fn a_dangling_leafref_reached_through_a_typedef_is_an_error() {
    // "pick" uses "removed-ref", a typedef whose own leafref points
    // at a node a deviation removes -- interpret_type clones the
    // typedef's own leafref_path/source_module into "pick"'s own
    // descriptor, so the ordinary node-level dangling-leafref check
    // (resolve_leafrefs's only leafref check now that a typedef's
    // own canonical entry never gets a target -- see
    // a_typedefs_own_canonical_entry_never_gets_a_leafref_target)
    // still catches it even though the leafref was declared on a
    // typedef, not directly on "pick".
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          typedef removed-ref {
            type leafref {
              path "/f:removed/f:name";
            }
          }

          container removed {
            leaf name {
              type string;
            }
          }

          leaf pick {
            type removed-ref;
          }

          deviation /f:removed {
            deviate not-supported;
          }
        }
    "#;
    let err = build(&[src.to_string()], &[], &[], &[], &[]).err().unwrap();
    assert!(err.0.contains("removed"), "{}", err.0);
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
    let err = build(&[src.to_string()], &[], &[], &[], &[]).err().unwrap();
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
    let err = build(&[src.to_string()], &[], &[], &[], &[]).err().unwrap();
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
    assert!(build(&[src.to_string()], &[], &[], &[], &[]).is_ok());
}

#[test]
fn a_deviated_away_node_is_unreachable_through_every_lookup() {
    // node_by_sid/node_by_schema_path/node_by_data_path must all
    // fail to find a node deviate not-supported removed -- none of
    // them should still walk to it via a stale sid_index entry or a
    // path that happens to still resolve.
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          container gone {
            leaf x {
              type string;
            }
          }
        }
    "#;
    let sid = r#"{
        "module-name": "foo",
        "module-revision": "2026-01-01",
        "items": [
            {"namespace": "data", "identifier": "/foo:gone", "sid": 5000}
        ]
    }"#;
    let present = build(&[src.to_string()], &[sid.to_string()], &[], &[], &[]).unwrap();
    assert!(present.node_by_sid(5000).is_some());
    assert!(present.node_by_schema_path("/foo:gone").is_some());
    assert!(present.node_by_data_path("/foo:gone").is_some());

    let src_deviated = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          container gone {
            leaf x {
              type string;
            }
          }

          deviation /f:gone {
            deviate not-supported;
          }
        }
    "#;
    let deviated = build(
        &[src_deviated.to_string()],
        &[sid.to_string()],
        &[],
        &[],
        &[],
    )
    .unwrap();
    assert!(deviated.node_by_sid(5000).is_none());
    assert!(deviated.node_by_schema_path("/foo:gone").is_none());
    assert!(deviated.node_by_data_path("/foo:gone").is_none());
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
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();

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
    let schema = build(&[src.to_string(), src2.to_string()], &[], &[], &[], &[]).unwrap();

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
fn yang_data_finds_it_but_node_by_schema_path_and_data_path_do_not() {
    // A yang-data root is its own separate document (RFC 8040 Sec.
    // 8), never reachable by walking any module's own schema tree --
    // so it has no schema-node-path and no data-node-path, and needs
    // its own dedicated entry point. The template is accept-listed,
    // same reason as a_yang_data_roots_status_defaults_to_current_too
    // above.
    let src = r#"
        module ietf-coreconf {
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
    let schema = build(
        &[src.to_string()],
        &[],
        &["ietf-coreconf:coreconf-error"],
        &[],
        &[],
    )
    .unwrap();
    let error_root = schema
        .yang_data()
        .find(|y| y.name == "coreconf-error")
        .unwrap()
        .root();
    assert_eq!(error_root.kw, "container");
    assert!(error_root.find_child("message").is_some());

    assert!(schema.node_by_schema_path("/ietf-coreconf:error").is_none());
    assert!(schema.node_by_data_path("/ietf-coreconf:error").is_none());
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
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    assert!(schema.node_by_schema_path("/foo:no-such-node").is_none());
    assert!(schema.node_by_schema_path("/no-such-module:bar").is_none());
    assert!(schema.node_by_data_path("/foo:no-such-node").is_none());
    assert!(schema.node_by_schema_path("").is_none());
    assert!(schema.node_by_data_path("").is_none());
}

#[test]
fn module_tracks_its_own_namespace_and_revision() {
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;
          revision 2026-01-01;

          leaf bar {
            type string;
          }
        }
    "#;
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    let module = schema.module_by_name("foo").unwrap();
    assert_eq!(module.namespace, "urn:foo");
    assert_eq!(module.revision.as_deref(), Some("2026-01-01"));
}

#[test]
fn module_revision_is_none_when_unstated() {
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          leaf bar {
            type string;
          }
        }
    "#;
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    let module = schema.module_by_name("foo").unwrap();
    assert_eq!(module.revision, None);
}

#[test]
fn a_module_whose_only_content_is_fully_deviated_away_is_still_implemented() {
    // "foo-dev" (no data nodes of its own, only a deviation) deviates
    // away "foo"'s only container. "foo-dev" is implemented by virtue
    // of its deviation statement (RFC 7950 5.6.5), and a server can't
    // implement deviations of a module it doesn't implement -- so
    // "foo" must stay implemented too, even with nothing of its own
    // left live. ietf-constrained-yang-library requires this as well:
    // a module's deviations can only be reported on its own `module`
    // entry, whose `deviation` leafrefs point at other `module`
    // entries, and `import-only-module` has no way to carry them.
    let foo = r#"
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
    let foo_dev = r#"
        module foo-dev {
          namespace "urn:foo-dev";
          prefix fd;

          import foo {
            prefix f;
          }

          deviation "/f:bar" {
            deviate not-supported;
          }
        }
    "#;
    let schema = build(&[foo.to_string(), foo_dev.to_string()], &[], &[], &[], &[]).unwrap();
    assert!(schema.module_by_name("foo").is_some());
    assert!(schema.module_by_name("foo-dev").is_some());
    assert!(schema.import_only_modules().all(|m| m.name != "foo"));
    assert_eq!(
        schema.module_by_name("foo").unwrap().root_nodes().count(),
        0
    );
}

#[test]
fn deviating_an_augmented_node_keeps_the_augmenting_module_implemented() {
    // "aug" contributes nothing but a leaf augmented into "host"'s
    // container, and "aug-dev" deviates that leaf away, then the
    // container itself -- so "aug" has neither a surviving node nor a
    // surviving augment target left. The deviated leaf's definition
    // belongs to "aug" (its own augment), not "host" (whose prefix the
    // deviation path starts with), so "aug" is a module being deviated
    // and must stay implemented.
    let host = r#"
        module host {
          namespace "urn:host";
          prefix h;

          container top {
            leaf own {
              type string;
            }
          }
        }
    "#;
    let aug = r#"
        module aug {
          namespace "urn:aug";
          prefix a;

          import host {
            prefix h;
          }

          augment "/h:top" {
            leaf extra {
              type string;
            }
          }
        }
    "#;
    let aug_dev = r#"
        module aug-dev {
          namespace "urn:aug-dev";
          prefix ad;

          import host {
            prefix h;
          }
          import aug {
            prefix a;
          }

          deviation "/h:top/a:extra" {
            deviate not-supported;
          }
          deviation "/h:top" {
            deviate not-supported;
          }
        }
    "#;
    let schema = build(
        &[host.to_string(), aug.to_string(), aug_dev.to_string()],
        &[],
        &[],
        &[],
        &[],
    )
    .unwrap();
    assert!(schema.module_by_name("aug").is_some());
    assert!(schema.import_only_modules().all(|m| m.name != "aug"));
    assert!(schema.node_by_schema_path("/host:top/aug:extra").is_none());
}

fn deviated_by_of(schema: &Schema, module: &str) -> Vec<String> {
    let mut names: Vec<String> = schema
        .module_by_name(module)
        .unwrap()
        .deviated_by
        .iter()
        .cloned()
        .collect();
    names.sort();
    names
}

#[test]
fn a_deviation_is_recorded_on_the_module_defining_the_deviated_node() {
    // "dev"'s deviation path starts in "host" but its target leaf is
    // defined by "aug"'s augment, so "aug" is the module "dev"
    // deviates -- "host" isn't deviated at all.
    let host = r#"
        module host {
          namespace "urn:host";
          prefix h;

          container top {
            leaf own {
              type string;
            }
          }
        }
    "#;
    let aug = r#"
        module aug {
          namespace "urn:aug";
          prefix a;

          import host {
            prefix h;
          }

          augment "/h:top" {
            leaf extra {
              type string;
            }
            leaf other {
              type string;
            }
          }
        }
    "#;
    let dev = r#"
        module dev {
          namespace "urn:dev";
          prefix d;

          import host {
            prefix h;
          }
          import aug {
            prefix a;
          }

          deviation "/h:top/a:extra" {
            deviate not-supported;
          }
        }
    "#;
    let schema = build(
        &[host.to_string(), aug.to_string(), dev.to_string()],
        &[],
        &[],
        &[],
        &[],
    )
    .unwrap();
    assert_eq!(deviated_by_of(&schema, "aug"), vec!["dev"]);
    assert!(deviated_by_of(&schema, "host").is_empty());
    assert!(deviated_by_of(&schema, "dev").is_empty());
}

#[test]
fn a_module_deviated_by_two_modules_lists_both_once() {
    // "dev-a" deviates two of "host"'s nodes, so it must be listed
    // once, not once per deviation statement.
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
          leaf c {
            type string;
          }
        }
    "#;
    let dev_a = r#"
        module dev-a {
          namespace "urn:dev-a";
          prefix da;

          import host {
            prefix h;
          }

          deviation "/h:a" {
            deviate not-supported;
          }
          deviation "/h:b" {
            deviate not-supported;
          }
        }
    "#;
    let dev_b = r#"
        module dev-b {
          namespace "urn:dev-b";
          prefix db;

          import host {
            prefix h;
          }

          deviation "/h:c" {
            deviate not-supported;
          }
        }
    "#;
    let schema = build(
        &[host.to_string(), dev_b.to_string(), dev_a.to_string()],
        &[],
        &[],
        &[],
        &[],
    )
    .unwrap();
    assert_eq!(deviated_by_of(&schema, "host"), vec!["dev-a", "dev-b"]);
}

#[test]
fn a_module_deviating_its_own_node_is_not_listed_as_deviating_itself() {
    // ietf-constrained-yang-library's module/deviation "MUST NOT
    // (directly or indirectly) refer to the module being deviated",
    // so the deviation is applied but not reported against the
    // module itself.
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          leaf keep {
            type string;
          }
          leaf gone {
            type string;
          }

          deviation /f:gone {
            deviate not-supported;
          }
        }
    "#;
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    assert!(schema.node_by_schema_path("/foo:gone").is_none());
    assert!(deviated_by_of(&schema, "foo").is_empty());
}

#[test]
fn an_augmenting_modules_own_live_nodes_make_it_implemented_even_with_empty_root_nodes() {
    // RFC 7950 4.2.8/7.17: a node placed into another module's tree
    // via augment is still owned by the augmenting module, not the
    // target -- "bar" has no top-level schema node of its own (its
    // only content is the augment), but the augmented leaf is real,
    // live data that "bar" implements, so "bar" must still be
    // implemented, not import-only.
    let foo = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          container top {
          }
        }
    "#;
    let bar = r#"
        module bar {
          namespace "urn:bar";
          prefix b;

          import foo {
            prefix f;
          }

          augment "/f:top" {
            leaf added {
              type string;
            }
          }
        }
    "#;
    let schema = build(&[foo.to_string(), bar.to_string()], &[], &[], &[], &[]).unwrap();
    assert!(schema.module_by_name("bar").is_some());
    assert!(schema.import_only_modules().all(|m| m.name != "bar"));
}

#[test]
fn a_module_whose_only_top_level_content_is_a_uses_is_implemented() {
    // "foo" declares no bare container/leaf/etc. of its own -- its
    // only top-level statement is `uses l:g;`, instantiating a
    // grouping defined in "lib". RFC 7950 7.13 treats a uses body
    // as copied in place, so "foo" ends up owning real data nodes
    // of its own (not "lib"'s) once the grouping is spliced in, and
    // must be implemented -- not import-only merely because its
    // own text has no bare data-definition keyword.
    let lib = r#"
        module lib {
          namespace "urn:lib";
          prefix l;

          grouping g {
            container c {
              leaf x {
                type string;
              }
            }
          }
        }
    "#;
    let foo = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          import lib {
            prefix l;
          }

          uses l:g;
        }
    "#;
    let schema = build(&[lib.to_string(), foo.to_string()], &[], &[], &[], &[]).unwrap();
    assert!(schema.module_by_name("foo").is_some());
    assert!(schema.import_only_modules().all(|m| m.name != "foo"));
    // And "lib" itself (declares only the grouping, never used
    // locally) must be import-only -- confirming the spliced
    // container/leaf are attributed to "foo", not misattributed
    // back to "lib".
    assert!(schema.module_by_name("lib").is_none());
    assert!(schema.import_only_modules().any(|m| m.name == "lib"));
}

#[test]
fn a_module_whose_only_content_is_an_accept_listed_yang_data_template_is_implemented() {
    // Whether a yang-data root is "protocol accessible" (this
    // device's own ietf-constrained-yang-library's wording for
    // what distinguishes a real module from an import-only one)
    // can't be derived from the YANG text or from whether fercc's
    // interpreter happens to build the node -- it's a real,
    // separately-maintained fact about what this device's wire
    // protocol actually serves (ui-tailoring.yaml's
    // yang-data-accept-list). "ietf-coreconf" declares nothing but
    // this one rc:yang-data template, with an inline container -- no
    // ordinary schema node, no augment, no deviation -- so it's
    // implemented only because "ietf-coreconf:error" is on that
    // accept-list.
    let src = r#"
        module ietf-coreconf {
          namespace "urn:foo";
          prefix f;

          rc:yang-data error {
            container error {
              leaf message {
                type string;
              }
            }
          }
        }
    "#;
    let schema = build(&[src.to_string()], &[], &["ietf-coreconf:error"], &[], &[]).unwrap();
    assert!(schema.module_by_name("ietf-coreconf").is_some());
    assert!(schema
        .import_only_modules()
        .all(|m| m.name != "ietf-coreconf"));
}

#[test]
fn a_module_whose_only_content_is_a_built_but_not_accept_listed_yang_data_template_is_import_only()
{
    // Same shape as the accept-listed case above, but under a
    // different template name that was never added to the
    // yang-data accept-list -- successfully building the
    // node is not enough on its own; the real-catalog
    // "ietf-restconf" case (whose own yang-errors/yang-api
    // templates build real nodes just like any other rc:yang-data
    // body, but must never flip this module to implemented, since
    // this device never implements RESTCONF). "bar" has nothing of
    // its own that resolves against anything "foo" declares -- its
    // leaf is plain "string", not even using "foo"'s own
    // yang-data-building extension -- so "foo" belongs in neither
    // set, matching
    // a_standalone_module_shaped_like_import_only_and_imported_by_
    // nothing_is_in_neither_set.
    let foo = r#"
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
    let bar = r#"
        module bar {
          namespace "urn:bar";
          prefix b;

          import foo {
            prefix f;
          }

          leaf real {
            type string;
          }
        }
    "#;
    let schema = build(&[foo.to_string(), bar.to_string()], &[], &[], &[], &[]).unwrap();
    assert!(schema.module_by_name("foo").is_none());
    assert!(schema.import_only_modules().all(|m| m.name != "foo"));
}

#[test]
fn a_module_declaring_only_a_live_identityrefs_base_identity_is_import_only() {
    // RFC 7950 7.18.2: identity derivation is irreflexive -- an
    // identity is never derived from itself -- so 9.10.2's "valid
    // values ... are any identities derived from" a live
    // identityref's base excludes the base itself. "bases" declares
    // only "animal" (the base "foo"'s leaf names), nothing actually
    // derived from it, so nothing in "bases" is ever a real wire
    // value -- it must stay import-only even though its own
    // identity is named directly by a live identityref's "base".
    // Confirmed against the real device: the real catalog's own
    // ieee802-dot1q-types (which declares only base identities like
    // "type-of-operation", reached by ieee802-dot1q-sched's live
    // "operation-name" leaf) is tagged import-only in the real
    // firmware's own generated classification
    // (lma_cc_module_ieee802_dot1q_types.c's
    // lma_cc_import_only_module_t), not implemented.
    let bases = r#"
        module bases {
          namespace "urn:bases";
          prefix bs;

          identity animal;
        }
    "#;
    let foo = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          import bases {
            prefix bs;
          }

          leaf pet {
            type identityref {
              base bs:animal;
            }
          }
        }
    "#;
    let schema = build(&[bases.to_string(), foo.to_string()], &[], &[], &[], &[]).unwrap();
    assert!(schema.module_by_name("bases").is_none());
    assert!(schema.import_only_modules().any(|m| m.name == "bases"));
}

#[test]
fn a_module_whose_identity_is_reached_only_through_another_identitys_derivation_is_implemented() {
    // Mirrors the real catalog's ieee802-dot1q-sched/-types case:
    // "derived" (declaring "dog", derived from "animal" in
    // "bases") is never itself named by any identityref's own
    // "base" -- only "animal" is -- but "dog" is still a legal
    // identityref value (RFC 7950 9.10.2's derived-identity closure,
    // already proven document-wide by
    // identities_only_lists_ones_reachable_from_an_identityref_
    // usage), so "derived" must count as implemented -- but "bases"
    // (declaring only the base identity itself, never a real wire
    // value per 7.18.2's irreflexivity -- see
    // a_module_declaring_only_a_live_identityrefs_base_identity_is_
    // import_only) must stay import-only even here, where something
    // real is genuinely implemented through it.
    let bases = r#"
        module bases {
          namespace "urn:bases";
          prefix bs;

          identity animal;
        }
    "#;
    let derived = r#"
        module derived {
          namespace "urn:derived";
          prefix d;

          import bases {
            prefix bs;
          }

          identity dog {
            base bs:animal;
          }
        }
    "#;
    let foo = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          import bases {
            prefix bs;
          }

          leaf pet {
            type identityref {
              base bs:animal;
            }
          }
        }
    "#;
    let schema = build(
        &[bases.to_string(), derived.to_string(), foo.to_string()],
        &[],
        &[],
        &[],
        &[],
    )
    .unwrap();
    assert!(schema.module_by_name("derived").is_some());
    assert!(schema.import_only_modules().all(|m| m.name != "derived"));
    assert!(schema.module_by_name("bases").is_none());
    assert!(schema.import_only_modules().any(|m| m.name == "bases"));
}

const ROUTING_LIKE: &str = r#"
    module rt {
      namespace "urn:rt";
      prefix rt;

      identity protocol;
      identity routing-protocol {
        base protocol;
      }
      identity direct {
        base routing-protocol;
      }
      identity static {
        base routing-protocol;
      }
      identity snooping {
        base protocol;
      }

      leaf protocol-type {
        type identityref {
          base protocol;
        }
      }
      leaf source-protocol {
        type identityref {
          base routing-protocol;
        }
      }
    }
"#;

fn value_names(schema: &Schema, leaf: &str) -> Vec<String> {
    let mut names: Vec<String> = schema
        .node_by_schema_path(leaf)
        .unwrap()
        .ty()
        .unwrap()
        .identity_values()
        .unwrap()
        .into_iter()
        .map(|i| i.name.clone())
        .collect();
    names.sort();
    names
}

#[test]
fn an_accept_list_is_exactly_its_bases_value_set_and_intermediates_are_bases_only() {
    // "routing-protocol" sits between the leaf's base "protocol" and the
    // accepted "static", so "static" refers to it as a base, but it
    // isn't itself a value.
    let accepted: &[&str] = &["rt:static", "rt:snooping"];
    let schema = build(
        &[ROUTING_LIKE.to_string()],
        &[],
        &[],
        &[("rt:protocol", accepted)],
        &[],
    )
    .unwrap();
    assert_eq!(
        value_names(&schema, "/rt:protocol-type"),
        vec!["snooping", "static"]
    );
    let routing_protocol = schema.identity_by_name("rt", "routing-protocol").unwrap();
    assert!(!routing_protocol.is_implemented());
    assert!(!schema
        .identity_by_name("rt", "protocol")
        .unwrap()
        .is_implemented());
}

#[test]
fn two_accept_lists_overlapping_on_an_intermediate_do_not_prune_each_other() {
    // "protocol-type" doesn't accept "direct", but "source-protocol"
    // does, and both leaves' value sets share the "routing-protocol"
    // intermediate. Each base's accept-list applies to that base alone.
    let protocol: &[&str] = &["rt:static", "rt:snooping"];
    let routing_protocol: &[&str] = &["rt:direct", "rt:static"];
    let schema = build(
        &[ROUTING_LIKE.to_string()],
        &[],
        &[],
        &[
            ("rt:protocol", protocol),
            ("rt:routing-protocol", routing_protocol),
        ],
        &[],
    )
    .unwrap();
    assert_eq!(
        value_names(&schema, "/rt:protocol-type"),
        vec!["snooping", "static"]
    );
    assert_eq!(
        value_names(&schema, "/rt:source-protocol"),
        vec!["direct", "static"]
    );
    assert!(schema
        .identity_by_name("rt", "direct")
        .unwrap()
        .is_implemented());
}

#[test]
fn without_an_accept_list_every_identity_derived_from_the_base_is_a_value() {
    let schema = build(&[ROUTING_LIKE.to_string()], &[], &[], &[], &[]).unwrap();
    assert_eq!(
        value_names(&schema, "/rt:protocol-type"),
        vec!["direct", "routing-protocol", "snooping", "static"]
    );
    assert!(schema
        .identity_by_name("rt", "routing-protocol")
        .unwrap()
        .is_implemented());
}

#[test]
fn an_identity_neither_implemented_nor_referred_to_as_a_base_is_dropped() {
    // Accepting only "snooping" leaves "routing-protocol" and its
    // children with nothing implemented beneath them, so they're
    // neither values nor referred to as a base by anything implemented.
    let accepted: &[&str] = &["rt:snooping"];
    let src = ROUTING_LIKE.replace(
        r#"      leaf source-protocol {
        type identityref {
          base routing-protocol;
        }
      }
"#,
        "",
    );
    let schema = build(&[src], &[], &[], &[("rt:protocol", accepted)], &[]).unwrap();
    let mut names: Vec<String> = schema.identities().map(|i| i.name.clone()).collect();
    names.sort();
    assert_eq!(names, vec!["protocol", "snooping"]);
}

#[test]
fn an_accept_list_entry_not_derived_from_its_base_is_a_schema_error() {
    let accepted: &[&str] = &["rt:snooping"];
    let err = build(
        &[ROUTING_LIKE.to_string()],
        &[],
        &[],
        &[("rt:routing-protocol", accepted)],
        &[],
    )
    .err()
    .expect("an accepted identity outside its base must fail");
    assert!(err.0.contains("not actually derived"), "{}", err.0);
}

const TWO_LEAVES_ONE_BASE: &str = r#"
    module af {
      namespace "urn:af";
      prefix af;

      identity address-family;
      identity ipv4 {
        base address-family;
      }
      identity ipv6 {
        base address-family;
      }
      identity mpls {
        base address-family;
      }

      container rib {
        leaf family {
          type identityref {
            base address-family;
          }
        }
      }
      container lldp {
        leaf subtype {
          type identityref {
            base address-family;
          }
        }
        leaf either {
          type union {
            type identityref {
              base address-family;
            }
            type string;
          }
        }
        leaf count {
          type uint8;
        }
      }
    }
"#;

fn identityref_values(schema: &Schema, leaf: &str) -> Vec<String> {
    let mut names: Vec<String> = schema
        .node_by_schema_path(leaf)
        .unwrap()
        .ty()
        .unwrap()
        .identity_values()
        .unwrap()
        .into_iter()
        .map(|i| i.name.clone())
        .collect();
    names.sort();
    names
}

#[test]
fn an_identityref_accept_list_narrows_only_the_leaf_it_names() {
    // Both leaves share the base, but only "rib/family" is listed, so
    // "lldp/subtype" keeps every derived identity.
    let accepted: &[&str] = &["af:ipv4"];
    let schema = build(
        &[TWO_LEAVES_ONE_BASE.to_string()],
        &[],
        &[],
        &[],
        &[("/af:rib/family", accepted)],
    )
    .unwrap();
    assert_eq!(identityref_values(&schema, "/af:rib/family"), vec!["ipv4"]);
    assert_eq!(
        identityref_values(&schema, "/af:lldp/subtype"),
        vec!["ipv4", "ipv6", "mpls"]
    );
}

#[test]
fn an_identityref_accept_list_replaces_the_bases_accept_list_for_its_leaf() {
    // The per-leaf entry decides alone: "ipv6" is outside the base's
    // list but still accepted for "rib/family", while every other leaf
    // with that base keeps the base's list.
    let by_base: &[&str] = &["af:ipv4", "af:mpls"];
    let by_leaf: &[&str] = &["af:ipv4", "af:ipv6"];
    let schema = build(
        &[TWO_LEAVES_ONE_BASE.to_string()],
        &[],
        &[],
        &[("af:address-family", by_base)],
        &[("/af:rib/family", by_leaf)],
    )
    .unwrap();
    assert_eq!(
        identityref_values(&schema, "/af:rib/family"),
        vec!["ipv4", "ipv6"]
    );
    assert_eq!(
        identityref_values(&schema, "/af:lldp/subtype"),
        vec!["ipv4", "mpls"]
    );
    let ipv6 = schema.identity_by_name("af", "ipv6").unwrap();
    assert!(ipv6.is_implemented());
}

#[test]
fn a_bases_accept_list_still_narrows_a_union_members_identityref() {
    let by_base: &[&str] = &["af:ipv4"];
    let schema = build(
        &[TWO_LEAVES_ONE_BASE.to_string()],
        &[],
        &[],
        &[("af:address-family", by_base)],
        &[],
    )
    .unwrap();
    let ty = schema
        .node_by_schema_path("/af:lldp/either")
        .unwrap()
        .ty()
        .unwrap();
    let member = ty.union_members().next().unwrap();
    let names: Vec<String> = member
        .identity_values()
        .unwrap()
        .into_iter()
        .map(|i| i.name.clone())
        .collect();
    assert_eq!(names, vec!["ipv4"]);
}

fn identityref_accept_list_error(leaf: &str, accepted: &[&str]) -> String {
    build(
        &[TWO_LEAVES_ONE_BASE.to_string()],
        &[],
        &[],
        &[],
        &[(leaf, accepted)],
    )
    .err()
    .expect("the identityref accept-list entry must be rejected")
    .0
}

#[test]
fn an_identityref_accept_list_narrows_a_leaf_list_typed_through_a_typedef() {
    // Like the device's mchp-vlan-port-type: the identityref comes from
    // a typedef, which another leaf also uses, and only the named
    // leaf-list is narrowed.
    let src = r#"
        module tdf {
          namespace "urn:tdf";
          prefix t;

          identity kind;
          identity a {
            base kind;
          }
          identity b {
            base kind;
          }

          typedef kind-ref {
            type identityref {
              base kind;
            }
          }

          leaf-list kinds {
            type kind-ref;
          }
          leaf other {
            type kind-ref;
          }
        }
    "#;
    let accepted: &[&str] = &["tdf:a"];
    let schema = build(
        &[src.to_string()],
        &[],
        &[],
        &[],
        &[("/tdf:kinds", accepted)],
    )
    .unwrap();
    let values = |leaf: &str| -> Vec<String> {
        let mut names: Vec<String> = schema
            .node_by_schema_path(leaf)
            .unwrap()
            .ty()
            .unwrap()
            .identity_values()
            .unwrap()
            .into_iter()
            .map(|i| i.name.clone())
            .collect();
        names.sort();
        names
    };
    assert_eq!(values("/tdf:kinds"), vec!["a"]);
    assert_eq!(values("/tdf:other"), vec!["a", "b"]);
}

#[test]
fn an_identityref_accept_list_entry_for_a_union_is_a_schema_error() {
    // The format can't say which union member an accepted identity
    // narrows, so a union isn't accepted at all.
    let e = identityref_accept_list_error("/af:lldp/either", &["af:ipv4"]);
    assert!(e.contains("is not an identityref"), "{e}");
}

#[test]
fn an_identityref_accept_list_entry_for_a_non_identityref_leaf_is_a_schema_error() {
    let e = identityref_accept_list_error("/af:lldp/count", &["af:ipv4"]);
    assert!(e.contains("is not an identityref"), "{e}");
}

#[test]
fn an_identityref_accept_list_entry_for_a_container_is_a_schema_error() {
    let e = identityref_accept_list_error("/af:lldp", &["af:ipv4"]);
    assert!(e.contains("not a leaf or leaf-list"), "{e}");
}

#[test]
fn an_identityref_accept_list_entry_for_an_unknown_path_is_a_schema_error() {
    let e = identityref_accept_list_error("/af:rib/nowhere", &["af:ipv4"]);
    assert!(
        e.contains("does not name an implemented schema node"),
        "{e}"
    );
}

#[test]
fn an_identityref_accept_list_entry_outside_the_leafs_base_is_a_schema_error() {
    let e = identityref_accept_list_error("/af:rib/family", &["af:address-family"]);
    assert!(e.contains("not actually derived"), "{e}");
}

#[test]
fn an_identityref_accept_list_entry_listed_twice_is_a_schema_error() {
    let accepted: &[&str] = &["af:ipv4"];
    let err = build(
        &[TWO_LEAVES_ONE_BASE.to_string()],
        &[],
        &[],
        &[],
        &[("/af:rib/family", accepted), ("/af:rib/family", accepted)],
    )
    .err()
    .expect("a leaf listed twice must be rejected");
    assert!(err.0.contains("listed more than once"), "{}", err.0);
}

#[test]
fn an_identityref_accept_list_entry_for_a_deviated_away_leaf_is_a_schema_error() {
    let dev = r#"
        module af-dev {
          namespace "urn:af-dev";
          prefix afd;

          import af {
            prefix af;
          }

          deviation "/af:rib/af:family" {
            deviate not-supported;
          }
        }
    "#;
    let accepted: &[&str] = &["af:ipv4"];
    let err = build(
        &[TWO_LEAVES_ONE_BASE.to_string(), dev.to_string()],
        &[],
        &[],
        &[],
        &[("/af:rib/family", accepted)],
    )
    .err()
    .expect("a deviated-away leaf must be rejected");
    assert!(
        err.0.contains("does not name an implemented schema node"),
        "{}",
        err.0
    );
}

const DEFAULTS: &str = r#"
    module dflt {
      namespace "urn:dflt";
      prefix d;

      typedef base-type {
        type uint8;
        default 5;
        units "seconds";
      }
      typedef narrowed {
        type base-type {
          range "1..10";
        }
      }
      typedef overridden {
        type base-type;
        default 7;
        units "minutes";
      }
      typedef either {
        type union {
          type base-type;
          type string;
        }
      }

      leaf plain {
        type narrowed;
      }
      leaf own {
        type narrowed;
        default 3;
        units "hours";
      }
      leaf required {
        type narrowed;
        mandatory true;
      }
      leaf via-override {
        type overridden;
      }
      leaf via-union {
        type either;
      }
      leaf-list several {
        type uint8;
        default 1;
        default 2;
      }
      leaf-list from-type {
        type base-type;
      }
      leaf-list at-least-one {
        type base-type;
        min-elements 1;
      }
      list entries {
        key "id";
        leaf id {
          type base-type;
        }
      }
    }
"#;

fn node<'a>(schema: &'a Schema, path: &str) -> NodeRef<'a> {
    schema.node_by_schema_path(path).unwrap()
}

#[test]
fn a_typedef_inherits_its_bases_default_and_units_unless_it_sets_its_own() {
    let schema = build(&[DEFAULTS.to_string()], &[], &[], &[], &[]).unwrap();
    let narrowed = schema.typedef_by_name("dflt", "narrowed").unwrap();
    assert_eq!(narrowed.default(), Some("5"));
    assert_eq!(narrowed.units(), Some("seconds"));
    let overridden = schema.typedef_by_name("dflt", "overridden").unwrap();
    assert_eq!(overridden.default(), Some("7"));
    assert_eq!(overridden.units(), Some("minutes"));
}

#[test]
fn a_union_typedef_does_not_inherit_a_members_default_or_units() {
    // RFC 7950 9.12: any default value or "units" property defined in
    // the member types is not inherited by the union type.
    let schema = build(&[DEFAULTS.to_string()], &[], &[], &[], &[]).unwrap();
    let either = schema.typedef_by_name("dflt", "either").unwrap();
    assert_eq!(either.default(), None);
    assert_eq!(either.units(), None);
    let via_union = node(&schema, "/dflt:via-union");
    assert!(via_union.defaults.is_empty());
    assert_eq!(via_union.units, None);
}

#[test]
fn a_leaf_takes_its_types_default_and_units_unless_it_has_its_own() {
    let schema = build(&[DEFAULTS.to_string()], &[], &[], &[], &[]).unwrap();
    let plain = node(&schema, "/dflt:plain");
    assert_eq!(plain.defaults, vec!["5"]);
    assert_eq!(plain.units.as_deref(), Some("seconds"));
    let own = node(&schema, "/dflt:own");
    assert_eq!(own.defaults, vec!["3"]);
    assert_eq!(own.units.as_deref(), Some("hours"));
    assert_eq!(node(&schema, "/dflt:via-override").defaults, vec!["7"]);
}

#[test]
fn a_mandatory_leaf_has_no_default_from_its_type() {
    // RFC 7950 7.6.1: the type's default applies only if the leaf is
    // not mandatory. Its units still come from the type.
    let schema = build(&[DEFAULTS.to_string()], &[], &[], &[], &[]).unwrap();
    let required = node(&schema, "/dflt:required");
    assert!(required.defaults.is_empty());
    assert_eq!(required.units.as_deref(), Some("seconds"));
}

#[test]
fn a_leaf_list_keeps_all_its_own_defaults_in_order() {
    let schema = build(&[DEFAULTS.to_string()], &[], &[], &[], &[]).unwrap();
    assert_eq!(node(&schema, "/dflt:several").defaults, vec!["1", "2"]);
}

#[test]
fn a_leaf_list_takes_one_instance_of_its_types_default_unless_min_elements_is_set() {
    // RFC 7950 7.7.2: one instance of the type's default, unless the
    // leaf-list has min-elements of at least 1.
    let schema = build(&[DEFAULTS.to_string()], &[], &[], &[], &[]).unwrap();
    assert_eq!(node(&schema, "/dflt:from-type").defaults, vec!["5"]);
    assert!(node(&schema, "/dflt:at-least-one").defaults.is_empty());
}

#[test]
fn a_list_keys_default_is_ignored() {
    // RFC 7950 7.8.2: any default values in the key leafs or their
    // types are ignored.
    let schema = build(&[DEFAULTS.to_string()], &[], &[], &[], &[]).unwrap();
    assert!(node(&schema, "/dflt:entries/id").defaults.is_empty());
}

#[test]
fn a_deviate_replace_type_recomputes_the_default_and_units_from_the_new_type() {
    let dev = r#"
        module dflt-dev {
          namespace "urn:dflt-dev";
          prefix dd;

          import dflt {
            prefix d;
          }

          deviation /d:plain {
            deviate replace {
              type d:overridden;
            }
          }
        }
    "#;
    let schema = build(&[DEFAULTS.to_string(), dev.to_string()], &[], &[], &[], &[]).unwrap();
    let plain = node(&schema, "/dflt:plain");
    assert_eq!(plain.defaults, vec!["7"]);
    assert_eq!(plain.units.as_deref(), Some("minutes"));
}

#[test]
fn require_instance_defaults_to_true_and_is_inherited_through_a_typedef() {
    let src = r#"
        module ri {
          namespace "urn:ri";
          prefix r;

          typedef loose-ref {
            type leafref {
              path "/r:target";
              require-instance false;
            }
          }

          leaf target {
            type string;
          }
          leaf strict {
            type leafref {
              path "/r:target";
            }
          }
          leaf loose {
            type loose-ref;
          }
          leaf any-node {
            type instance-identifier;
          }
          leaf any-loose-node {
            type instance-identifier {
              require-instance false;
            }
          }
        }
    "#;
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    let ri = |path: &str| node(&schema, path).ty().unwrap().require_instance;
    assert!(ri("/ri:strict"));
    assert!(!ri("/ri:loose"));
    assert!(ri("/ri:any-node"));
    assert!(!ri("/ri:any-loose-node"));
}

#[test]
fn a_patterns_invert_match_modifier_is_read() {
    let src = r#"
        module pat {
          namespace "urn:pat";
          prefix p;

          leaf name {
            type string {
              pattern "[a-z]+";
              pattern "x.*" {
                modifier invert-match;
              }
            }
          }
        }
    "#;
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    let ty = node(&schema, "/pat:name").ty().unwrap();
    let flags: Vec<(String, bool)> = ty
        .patterns
        .iter()
        .map(|p| (p.regex.clone(), p.invert_match))
        .collect();
    assert_eq!(
        flags,
        vec![("[a-z]+".to_string(), false), ("x.*".to_string(), true)]
    );
}

const ERROR_INFO: &str = r#"
    module emsg {
      namespace "urn:emsg";
      prefix e;

      typedef percent {
        type uint8 {
          range "0..100" {
            error-message "must be a percentage";
            error-app-tag "not-a-percentage";
          }
        }
      }
      typedef half {
        type percent {
          range "0..50";
        }
      }
      typedef small-half {
        type percent {
          range "0..10" {
            error-message "must be at most 10";
          }
        }
      }

      leaf inherited {
        type half;
      }
      leaf overridden {
        type small-half;
      }
      leaf name {
        type string {
          length "1..8" {
            error-app-tag "bad-length";
          }
          pattern "[a-z]+" {
            error-message "lower case only";
          }
          pattern "x.*" {
            modifier invert-match;
            error-app-tag "starts-with-x";
          }
        }
      }
    }
"#;

#[test]
fn a_narrowing_range_without_its_own_error_info_keeps_the_inherited_one() {
    let schema = build(&[ERROR_INFO.to_string()], &[], &[], &[], &[]).unwrap();
    let ty = node(&schema, "/emsg:inherited").ty().unwrap();
    assert_eq!(ty.ranges, vec![(0, 50)]);
    assert_eq!(
        ty.range_error.error_message.as_deref(),
        Some("must be a percentage")
    );
    assert_eq!(
        ty.range_error.error_app_tag.as_deref(),
        Some("not-a-percentage")
    );
}

#[test]
fn a_narrowing_range_replaces_only_the_error_info_it_declares() {
    // Matches libyang, which rewrites each of error-message and
    // error-app-tag as the type chain is processed, only where the
    // narrowing statement declares it.
    let schema = build(&[ERROR_INFO.to_string()], &[], &[], &[], &[]).unwrap();
    let ty = node(&schema, "/emsg:overridden").ty().unwrap();
    assert_eq!(
        ty.range_error.error_message.as_deref(),
        Some("must be at most 10")
    );
    assert_eq!(
        ty.range_error.error_app_tag.as_deref(),
        Some("not-a-percentage")
    );
}

#[test]
fn each_pattern_and_a_length_carry_their_own_error_info() {
    let schema = build(&[ERROR_INFO.to_string()], &[], &[], &[], &[]).unwrap();
    let ty = node(&schema, "/emsg:name").ty().unwrap();
    assert_eq!(ty.range_error.error_message, None);
    assert_eq!(ty.range_error.error_app_tag.as_deref(), Some("bad-length"));
    let errors: Vec<(Option<&str>, Option<&str>)> = ty
        .patterns
        .iter()
        .map(|p| {
            (
                p.error.error_message.as_deref(),
                p.error.error_app_tag.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        errors,
        vec![
            (Some("lower case only"), None),
            (None, Some("starts-with-x"))
        ]
    );
}

#[test]
fn modules_import_only_modules_and_structure_roots_iterate_in_name_order() {
    // None of these has a meaningful declaration order of its own, so
    // they come out sorted, whichever order their sources were given in.
    let lib = |name: &str| {
        format!(
            r#"
            module {name} {{
              namespace "urn:{name}";
              prefix {name};

              typedef t {{
                type string;
              }}
            }}
        "#
        )
    };
    let zeta = r#"
        module zeta {
          namespace "urn:zeta";
          prefix z;

          import ietf-yang-structure-ext {
            prefix sx;
          }
          import lib-a {
            prefix la;
            revision-date 2026-02-01;
          }
          import lib-b {
            prefix lb;
          }

          sx:structure zeta-doc {
            leaf x {
              type string;
            }
          }

          leaf a {
            type la:t;
          }
          leaf b {
            type lb:t;
          }
        }
    "#;
    let alpha = r#"
        module alpha {
          namespace "urn:alpha";
          prefix a;

          import ietf-yang-structure-ext {
            prefix sx;
          }
          import lib-a {
            prefix la;
            revision-date 2026-01-01;
          }

          sx:structure alpha-doc {
            leaf x {
              type string;
            }
          }

          leaf c {
            type la:t;
          }
        }
    "#;
    let filler_module = |name: &str| {
        format!(
            r#"
            module {name} {{
              namespace "urn:{name}";
              prefix {name};

              import ietf-yang-structure-ext {{
                prefix sx;
              }}

              sx:structure {name}-doc {{
                leaf x {{
                  type string;
                }}
              }}

              leaf {name}-leaf {{
                type string;
              }}
            }}
        "#
        )
    };
    let mut sources = vec![
        STRUCTURE_EXT.to_string(),
        lib("lib-b"),
        lib("lib-a"),
        zeta.to_string(),
        alpha.to_string(),
    ];
    // Enough implemented modules and structures that an unsorted map
    // order matching the sorted one by chance is out of the question.
    let fillers: Vec<String> = (0..12).map(|i| format!("m{i:02}")).collect();
    sources.extend(fillers.iter().rev().map(|name| filler_module(name)));
    let schema = build(&sources, &[], &[], &[], &[]).unwrap();
    let modules: Vec<String> = schema.modules().map(|m| m.name.clone()).collect();
    let mut expected: Vec<String> = vec!["alpha".to_string(), "zeta".to_string()];
    expected.extend(fillers.iter().cloned());
    expected.sort();
    assert_eq!(modules, expected);
    let import_only: Vec<(String, Option<String>)> = schema
        .import_only_modules()
        .map(|m| (m.name.clone(), m.import_revision_date.clone()))
        .collect();
    assert_eq!(
        import_only,
        vec![
            ("lib-a".to_string(), Some("2026-01-01".to_string())),
            ("lib-a".to_string(), Some("2026-02-01".to_string())),
            ("lib-b".to_string(), None),
        ]
    );
    let structures: Vec<String> = schema.structure_roots().map(|r| r.name.clone()).collect();
    let mut expected_structures: Vec<String> =
        vec!["alpha:alpha-doc".to_string(), "zeta:zeta-doc".to_string()];
    for name in &fillers {
        expected_structures.push(format!("{name}:{name}-doc"));
    }
    expected_structures.sort();
    assert_eq!(structures, expected_structures);
}

#[test]
fn the_schema_does_not_depend_on_the_order_its_modules_are_given_in() {
    // Two modules augment the same container and neither imports the
    // other, so nothing orders them; their nodes are appended to its
    // children in a fixed order regardless of which source came first.
    let host = r#"
        module host {
          namespace "urn:host";
          prefix h;

          container top {
            leaf own {
              type string;
            }
          }
        }
    "#;
    let augmenter = |name: &str| {
        format!(
            r#"
            module {name} {{
              namespace "urn:{name}";
              prefix {name};

              import host {{
                prefix h;
              }}

              augment "/h:top" {{
                leaf {name}-leaf {{
                  type string;
                }}
              }}
            }}
        "#
        )
    };
    let sources = vec![host.to_string(), augmenter("aaa"), augmenter("zzz")];
    let shape = |sources: &[String]| {
        let schema = build(sources, &[], &[], &[], &[]).unwrap();
        let top = schema.node_by_schema_path("/host:top").unwrap();
        top.children()
            .map(|c| c.name.clone())
            .collect::<Vec<String>>()
    };
    let mut reversed = sources.clone();
    reversed.reverse();
    assert_eq!(shape(&sources), shape(&reversed));
}

#[test]
fn a_yang_data_template_not_on_the_accept_list_is_absent_entirely() {
    // schema.rs must not distinguish between "implemented
    // yang-data" and "yang-data" -- the only yang-data a Schema
    // exposes at all, through any accessor, is the accept-listed
    // kind. "foo"'s template is successfully built (a real,
    // well-formed container) but isn't on the accept-list, so it
    // must not merely fail to count toward "implemented" -- it must
    // not exist in yang_data() at all.
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
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    assert_eq!(schema.yang_data().count(), 0);
}

#[test]
fn a_yang_data_bodys_uses_is_spliced_just_like_a_direct_container() {
    // RFC 7950 7.13 treats a uses body as copied in place, so a
    // yang-data body of `uses reusable;` must splice through to the
    // grouping's own container exactly like a direct one would --
    // the real-catalog "ietf-restconf" case: `rc:yang-data
    // yang-errors { uses errors; }`. The template is accept-listed so
    // the spliced result is actually observable through the public
    // API (which exposes only accept-listed yang-data templates -- see
    // a_yang_data_template_not_on_the_accept_list_is_absent_entirely).
    let src = r#"
        module ietf-coreconf {
          namespace "urn:foo";
          prefix f;

          grouping reusable {
            container error {
              leaf message {
                type string;
              }
            }
          }

          rc:yang-data error {
            uses reusable;
          }
        }
    "#;
    let schema = build(&[src.to_string()], &[], &["ietf-coreconf:error"], &[], &[]).unwrap();
    let root = schema
        .yang_data()
        .find(|y| y.name == "error")
        .expect("uses-spliced yang-data container must be built")
        .root();
    assert_eq!(root.children().count(), 1);
    assert!(schema.module_by_name("ietf-coreconf").is_some());
    assert!(schema
        .import_only_modules()
        .all(|m| m.name != "ietf-coreconf"));
}

#[test]
fn a_yang_data_body_with_more_than_one_data_node_is_rejected() {
    // RFC 8040 8: "It MUST contain data definition statements that
    // result in exactly one container data node definition."
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          rc:yang-data two-nodes {
            container a {
            }
            container b {
            }
          }
        }
    "#;
    assert!(build(&[src.to_string()], &[], &[], &[], &[]).is_err());
}

#[test]
fn a_yang_data_body_whose_one_node_is_not_a_container_is_rejected() {
    // RFC 8040 8's "exactly one container" requirement rules out a
    // body whose single top-level node is some other kind, e.g. a
    // bare leaf.
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          rc:yang-data not-a-container {
            leaf x {
              type string;
            }
          }
        }
    "#;
    assert!(build(&[src.to_string()], &[], &[], &[], &[]).is_err());
}

#[test]
fn an_accept_listed_yang_data_name_that_names_no_template_is_a_schema_error() {
    // A typo in ui-tailoring.yaml's yang-data-accept-list must not
    // silently implement nothing.
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
    let err = build(&[src.to_string()], &[], &["foo:no-such-template"], &[], &[])
        .err()
        .expect("an accept-listed name with no matching template must fail");
    assert!(err.0.contains("\"foo:no-such-template\""), "{}", err.0);
}

#[test]
fn the_yang_data_accept_list_names_a_template_not_its_container() {
    // The template's container ("error") is the root's name, not the
    // template's -- accept-listing "foo:error" must not match the
    // "error-template" template.
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
    assert!(build(&[src.to_string()], &[], &["foo:error"], &[], &[]).is_err());
    let schema = build(&[src.to_string()], &[], &["foo:error-template"], &[], &[]).unwrap();
    let template = schema.yang_data().next().unwrap();
    assert_eq!(template.module, "foo");
    assert_eq!(template.name, "error-template");
    assert_eq!(template.root().path(), "/foo:error");
}

#[test]
fn a_yang_data_template_defined_twice_in_one_module_is_a_schema_error() {
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          rc:yang-data error-template {
            container first {
            }
          }
          rc:yang-data error-template {
            container second {
            }
          }
        }
    "#;
    let err = build(&[src.to_string()], &[], &[], &[], &[])
        .err()
        .expect("a duplicate template name must fail");
    assert!(err.0.contains("\"error-template\""), "{}", err.0);
    assert!(err.0.contains("more than once"), "{}", err.0);
}

#[test]
fn two_yang_data_templates_with_the_same_container_name_in_one_module_are_a_schema_error() {
    // Both containers would be addressed as "/foo:error", in schema-nodes
    // and in the module's .sid file alike.
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          rc:yang-data first-template {
            container error {
            }
          }
          rc:yang-data second-template {
            container error {
            }
          }
        }
    "#;
    let err = build(&[src.to_string()], &[], &[], &[], &[])
        .err()
        .expect("two same-named yang-data containers must fail");
    assert!(err.0.contains("\"second-template\""), "{}", err.0);
    assert!(err.0.contains("collides"), "{}", err.0);
}

#[test]
fn a_yang_data_container_colliding_with_a_top_level_data_node_is_a_schema_error() {
    // Both would be addressed as "/foo:error" in schema-nodes. The data
    // node is declared after the template so the check can't depend on
    // declaration order.
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          rc:yang-data error-template {
            container error {
            }
          }
          container error {
          }
        }
    "#;
    let err = build(&[src.to_string()], &[], &[], &[], &[])
        .err()
        .expect("a yang-data container colliding with a data node must fail");
    assert!(err.0.contains("\"error\""), "{}", err.0);
    assert!(err.0.contains("collides"), "{}", err.0);
}

#[test]
fn same_named_yang_data_containers_in_different_modules_are_independent() {
    let foo = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          rc:yang-data error-template {
            container error {
            }
          }
        }
    "#;
    let bar = r#"
        module bar {
          namespace "urn:bar";
          prefix b;

          rc:yang-data error-template {
            container error {
            }
          }
        }
    "#;
    let schema = build(
        &[foo.to_string(), bar.to_string()],
        &[],
        &["foo:error-template", "bar:error-template"],
        &[],
        &[],
    )
    .unwrap();
    let mut roots: Vec<String> = schema.yang_data().map(|y| y.root().path()).collect();
    roots.sort();
    assert_eq!(roots, vec!["/bar:error", "/foo:error"]);
}

#[test]
fn no_module_name_is_ever_listed_in_both_modules_and_import_only_modules() {
    // modules() and import_only_modules() must partition disjointly
    // -- a name identifying something structurally different
    // (schema-root-nodes/data-root-nodes vs. none) can never satisfy both.
    let lib = r#"
        module lib {
          namespace "urn:lib";
          prefix l;

          typedef percent {
            type uint8 {
              range "0..100";
            }
          }
        }
    "#;
    let mid = r#"
        module mid {
          namespace "urn:mid";
          prefix m;

          import lib {
            prefix l;
          }

          typedef percent {
            type l:percent;
          }
        }
    "#;
    let foo = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          import mid {
            prefix m;
          }

          leaf bar {
            type m:percent;
          }
        }
    "#;
    let schema = build(
        &[lib.to_string(), mid.to_string(), foo.to_string()],
        &[],
        &[],
        &[],
        &[],
    )
    .unwrap();
    let implemented: std::collections::HashSet<String> =
        schema.modules().map(|m| m.name.clone()).collect();
    let import_only: std::collections::HashSet<String> = schema
        .import_only_modules()
        .map(|m| m.name.clone())
        .collect();
    assert!(
        implemented.is_disjoint(&import_only),
        "overlap: {:?}",
        implemented.intersection(&import_only).collect::<Vec<_>>()
    );
    assert_eq!(
        implemented,
        std::collections::HashSet::from(["foo".to_string()])
    );
    assert_eq!(
        import_only,
        std::collections::HashSet::from(["lib".to_string(), "mid".to_string()])
    );
}

#[test]
fn an_import_only_module_needed_by_an_implemented_one_is_listed_separately() {
    // "lib" declares nothing but a typedef -- a textbook import-only
    // module (RFC 8525's import-only-module concept): no top-level
    // schema node, uses, augment, deviation, or yang-data of its
    // own. "foo" genuinely implements something and imports "lib",
    // so "lib" must appear in import_only_modules, not modules --
    // and must stay out of modules() entirely, since it has no
    // schema-root-nodes/data-root-nodes to speak of.
    let lib = r#"
        module lib {
          namespace "urn:lib";
          prefix l;

          typedef percent {
            type uint8 {
              range "0..100";
            }
          }
        }
    "#;
    let foo = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          import lib {
            prefix l;
          }

          leaf bar {
            type l:percent;
          }
        }
    "#;
    let schema = build(&[lib.to_string(), foo.to_string()], &[], &[], &[], &[]).unwrap();
    assert!(schema.module_by_name("lib").is_none());
    assert!(schema.module_by_name("foo").is_some());

    let import_only: Vec<&ImportOnlyModule> = schema.import_only_modules().collect();
    assert_eq!(import_only.len(), 1);
    assert_eq!(import_only[0].name, "lib");
    assert_eq!(import_only[0].namespace, "urn:lib");
    assert_eq!(import_only[0].import_revision_date, None);
}

#[test]
fn a_standalone_module_shaped_like_import_only_and_imported_by_nothing_is_in_neither_set() {
    // "lib" doesn't implement any data node/RPC/action/
    // notification/deviation of its own (RFC 7950 5.6.5's positive
    // criterion for "implements" never applies to it), so it's not
    // in modules() -- but nothing anywhere imports it either, so
    // there's no importer relationship to list it under in
    // import_only_modules() (RFC 8525's import-only-module concept
    // requires actually being imported). It belongs in neither.
    let src = r#"
        module lib {
          namespace "urn:lib";
          prefix l;

          typedef percent {
            type uint8 {
              range "0..100";
            }
          }
        }
    "#;
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    assert!(schema.module_by_name("lib").is_none());
    assert_eq!(schema.import_only_modules().count(), 0);
}

#[test]
fn a_chain_of_import_only_modules_nobody_implemented_imports_is_listed_nowhere() {
    // "lib" is import-only-shaped and imported only by "mid", which
    // is itself import-only-shaped and imported by nothing at all
    // -- no genuinely-implemented module ever reaches either, so
    // the transitive walk (which only starts from implemented
    // modules) never visits "mid", and therefore never visits
    // "lib" either. Neither has an importer relationship to a real
    // module, so both belong in neither modules() nor
    // import_only_modules() -- matching ietf-yang-library: it only
    // reports modules the server actually uses.
    let lib = r#"
        module lib {
          namespace "urn:lib";
          prefix l;

          typedef percent {
            type uint8 {
              range "0..100";
            }
          }
        }
    "#;
    let mid = r#"
        module mid {
          namespace "urn:mid";
          prefix m;

          import lib {
            prefix l;
          }

          typedef percent {
            type l:percent;
          }
        }
    "#;
    let schema = build(&[lib.to_string(), mid.to_string()], &[], &[], &[], &[]).unwrap();
    assert!(schema.module_by_name("lib").is_none());
    assert!(schema.module_by_name("mid").is_none());
    assert_eq!(schema.import_only_modules().count(), 0);
}

#[test]
fn an_import_only_module_reached_through_a_chain_of_import_only_modules_is_still_listed() {
    // "lib" is import-only and reached only via "mid" (also
    // import-only), but "mid" itself is imported by "foo", which is
    // genuinely implemented -- the whole chain is needed, so both
    // "lib" and "mid" must be listed.
    let lib = r#"
        module lib {
          namespace "urn:lib";
          prefix l;

          typedef percent {
            type uint8 {
              range "0..100";
            }
          }
        }
    "#;
    let mid = r#"
        module mid {
          namespace "urn:mid";
          prefix m;

          import lib {
            prefix l;
          }

          typedef percent {
            type l:percent;
          }
        }
    "#;
    let foo = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          import mid {
            prefix m;
          }

          leaf bar {
            type m:percent;
          }
        }
    "#;
    let schema = build(
        &[lib.to_string(), mid.to_string(), foo.to_string()],
        &[],
        &[],
        &[],
        &[],
    )
    .unwrap();
    let names: std::collections::HashSet<String> = schema
        .import_only_modules()
        .map(|m| m.name.clone())
        .collect();
    assert_eq!(
        names,
        std::collections::HashSet::from(["lib".to_string(), "mid".to_string()])
    );
}

#[test]
fn an_import_only_modules_revision_date_comes_from_the_importers_own_import_statement() {
    // The RFC 8525 key is "name revision", where "revision" is the
    // revision-date on the IMPORTING module's own import statement
    // -- not the imported module's own revision statement (which
    // "lib" doesn't even have here).
    let lib = r#"
        module lib {
          namespace "urn:lib";
          prefix l;

          typedef percent {
            type uint8 {
              range "0..100";
            }
          }
        }
    "#;
    let foo = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          import lib {
            prefix l;
            revision-date 2025-01-01;
          }

          leaf bar {
            type l:percent;
          }
        }
    "#;
    let schema = build(&[lib.to_string(), foo.to_string()], &[], &[], &[], &[]).unwrap();
    let import_only: Vec<&ImportOnlyModule> = schema.import_only_modules().collect();
    assert_eq!(import_only.len(), 1);
    assert_eq!(
        import_only[0].import_revision_date.as_deref(),
        Some("2025-01-01")
    );
}

#[test]
fn two_importers_with_different_revision_dates_produce_two_distinct_entries() {
    // RFC 8525's own description: "Multiple entries for the same
    // module name MAY exist. This can occur if multiple modules
    // import the same module but specify different revision dates
    // in the import statements."
    let lib = r#"
        module lib {
          namespace "urn:lib";
          prefix l;

          typedef percent {
            type uint8 {
              range "0..100";
            }
          }
        }
    "#;
    let foo = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          import lib {
            prefix l;
            revision-date 2025-01-01;
          }

          leaf bar {
            type l:percent;
          }
        }
    "#;
    let baz = r#"
        module baz {
          namespace "urn:baz";
          prefix b;

          import lib {
            prefix l;
            revision-date 2026-01-01;
          }

          leaf qux {
            type l:percent;
          }
        }
    "#;
    let schema = build(
        &[lib.to_string(), foo.to_string(), baz.to_string()],
        &[],
        &[],
        &[],
        &[],
    )
    .unwrap();
    let mut dates: Vec<Option<String>> = schema
        .import_only_modules()
        .map(|m| m.import_revision_date.clone())
        .collect();
    dates.sort();
    assert_eq!(
        dates,
        vec![
            Some("2025-01-01".to_string()),
            Some("2026-01-01".to_string())
        ]
    );
}

#[test]
fn a_module_imported_only_for_an_unused_grouping_is_listed_nowhere() {
    // "lib" declares a grouping, but "foo" (which imports "lib")
    // never actually `uses` it -- an import statement alone, with
    // no real reference to anything "lib" declares, must not make
    // "lib" import-only.
    let lib = r#"
        module lib {
          namespace "urn:lib";
          prefix l;

          grouping widget {
            leaf name {
              type string;
            }
          }
        }
    "#;
    let foo = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          import lib {
            prefix l;
          }

          leaf bar {
            type string;
          }
        }
    "#;
    let schema = build(&[lib.to_string(), foo.to_string()], &[], &[], &[], &[]).unwrap();
    assert!(schema.module_by_name("lib").is_none());
    assert!(schema.import_only_modules().all(|m| m.name != "lib"));
}

#[test]
fn a_module_whose_grouping_is_actually_spliced_in_by_a_live_uses_is_import_only() {
    // Same "lib" as above, but "foo" genuinely `uses` its grouping
    // on a live, surviving node -- "lib" must now be listed.
    let lib = r#"
        module lib {
          namespace "urn:lib";
          prefix l;

          grouping widget {
            leaf name {
              type string;
            }
          }
        }
    "#;
    let foo = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          import lib {
            prefix l;
          }

          container bar {
            uses l:widget;
          }
        }
    "#;
    let schema = build(&[lib.to_string(), foo.to_string()], &[], &[], &[], &[]).unwrap();
    assert!(schema.module_by_name("lib").is_none());
    assert!(schema.import_only_modules().any(|m| m.name == "lib"));
}

#[test]
fn a_grouping_spliced_in_by_a_since_deviated_away_uses_is_listed_nowhere() {
    // Mirrors the typedef/identity liveness rule: "foo"'s own
    // "bar" container genuinely `uses` "lib"'s grouping, but
    // "foo-dev" deviates "bar" away entirely -- the splice never
    // actually survives into the live tree, so "lib" must not be
    // listed either, the same way an unreachable typedef/identity
    // isn't.
    let lib = r#"
        module lib {
          namespace "urn:lib";
          prefix l;

          grouping widget {
            leaf name {
              type string;
            }
          }
        }
    "#;
    let foo = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          import lib {
            prefix l;
          }

          container bar {
            uses l:widget;
          }
        }
    "#;
    let foo_dev = r#"
        module foo-dev {
          namespace "urn:foo-dev";
          prefix fd;

          import foo {
            prefix f;
          }

          deviation "/f:bar" {
            deviate not-supported;
          }
        }
    "#;
    let schema = build(
        &[lib.to_string(), foo.to_string(), foo_dev.to_string()],
        &[],
        &[],
        &[],
        &[],
    )
    .unwrap();
    assert!(schema.import_only_modules().all(|m| m.name != "lib"));
}

#[test]
fn a_module_imported_only_for_an_unused_extension_is_listed_nowhere() {
    // "lib" declares an extension, but "foo" (which imports "lib")
    // never actually invokes it on anything -- an import statement
    // alone must not make "lib" import-only.
    let lib = r#"
        module lib {
          namespace "urn:lib";
          prefix l;

          extension tag {
            description "An inert annotation.";
          }
        }
    "#;
    let foo = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          import lib {
            prefix l;
          }

          leaf bar {
            type string;
          }
        }
    "#;
    let schema = build(&[lib.to_string(), foo.to_string()], &[], &[], &[], &[]).unwrap();
    assert!(schema.module_by_name("lib").is_none());
    assert!(schema.import_only_modules().all(|m| m.name != "lib"));
}

#[test]
fn a_module_whose_extension_is_actually_invoked_on_a_live_node_is_import_only() {
    // Same "lib" as above, but "foo" genuinely invokes "l:tag" on
    // a live, surviving leaf -- "lib" must now be listed.
    let lib = r#"
        module lib {
          namespace "urn:lib";
          prefix l;

          extension tag {
            description "An inert annotation.";
          }
        }
    "#;
    let foo = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          import lib {
            prefix l;
          }

          leaf bar {
            type string;
            l:tag;
          }
        }
    "#;
    let schema = build(&[lib.to_string(), foo.to_string()], &[], &[], &[], &[]).unwrap();
    assert!(schema.module_by_name("lib").is_none());
    assert!(schema.import_only_modules().any(|m| m.name == "lib"));
}

#[test]
fn an_extension_invoked_on_a_since_deviated_away_node_is_listed_nowhere() {
    // Mirrors the grouping case: "foo"'s own "bar" leaf genuinely
    // invokes "l:tag", but "foo-dev" deviates "bar" away entirely
    // -- the invocation never survives into the live tree, so
    // "lib" must not be listed either.
    let lib = r#"
        module lib {
          namespace "urn:lib";
          prefix l;

          extension tag {
            description "An inert annotation.";
          }
        }
    "#;
    let foo = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          import lib {
            prefix l;
          }

          container top {
            leaf bar {
              type string;
              l:tag;
            }
          }
        }
    "#;
    let foo_dev = r#"
        module foo-dev {
          namespace "urn:foo-dev";
          prefix fd;

          import foo {
            prefix f;
          }

          deviation "/f:top/f:bar" {
            deviate not-supported;
          }
        }
    "#;
    let schema = build(
        &[lib.to_string(), foo.to_string(), foo_dev.to_string()],
        &[],
        &[],
        &[],
        &[],
    )
    .unwrap();
    assert!(schema.import_only_modules().all(|m| m.name != "lib"));
}

#[test]
fn a_missing_namespace_statement_is_a_schema_error() {
    let src = r#"
        module foo {
          prefix f;
        }
    "#;
    let err = build(&[src.to_string()], &[], &[], &[], &[]).err().unwrap();
    assert!(err.0.contains("namespace"), "{}", err.0);
}

#[test]
fn list_keys_resolve_against_their_own_direct_children() {
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          list entry {
            key "name index";
            leaf name {
              type string;
            }
            leaf index {
              type uint8;
            }
            leaf other {
              type string;
            }
          }
        }
    "#;
    let schema = build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    let entry = schema.node_by_data_path("/foo:entry").unwrap();
    let key_names: Vec<String> = entry.keys().map(|k| k.local_name().to_string()).collect();
    assert_eq!(key_names, vec!["name", "index"]);
}

#[test]
fn a_key_naming_something_other_than_a_direct_child_is_a_schema_error() {
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          list entry {
            key "missing";
            leaf name {
              type string;
            }
          }
        }
    "#;
    let err = build(&[src.to_string()], &[], &[], &[], &[]).err().unwrap();
    assert!(err.0.contains("missing"), "{}", err.0);
    assert!(err.0.contains("entry"), "{}", err.0);
}

#[test]
fn a_structures_own_typedefs_and_groupings_resolve_from_inside_it() {
    // RFC 8791 allows typedef and grouping statements directly inside
    // `sx:structure`, scoped to that structure: "doc" declares both, and
    // its own nodes must resolve against them -- "name" gets its type
    // from the structure's own "label" typedef, through the structure's
    // own "entry-fields" grouping. Same names at module level must not be
    // what they resolve to, so the module's own versions differ.
    let sx = r#"
        module ietf-yang-structure-ext {
          namespace "urn:ietf:params:xml:ns:yang:ietf-yang-structure-ext";
          prefix sx;

          extension structure {
            argument name;
          }
        }
    "#;
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          import ietf-yang-structure-ext {
            prefix sx;
          }

          typedef label {
            type uint8;
          }

          sx:structure doc {
            typedef label {
              type string;
            }
            grouping entry-fields {
              leaf name {
                type label;
              }
            }
            list entry {
              uses entry-fields;
            }
          }
        }
    "#;
    let schema = build(&[sx.to_string(), src.to_string()], &[], &[], &[], &[]).unwrap();
    let doc = schema
        .structure_roots()
        .find(|r| r.name == "foo:doc")
        .unwrap();
    let name = doc.find_child("entry").unwrap().find_child("name").unwrap();
    let ty = name.ty().unwrap();
    assert_eq!(ty.builtin, Builtin::String);
    assert_eq!(ty.base().unwrap().name, "label");
    assert_eq!(ty.base().unwrap().scope, "/foo:doc");
}

const STRUCTURE_EXT: &str = r#"
    module ietf-yang-structure-ext {
      namespace "urn:ietf:params:xml:ns:yang:ietf-yang-structure-ext";
      prefix sx;

      extension structure {
        argument name;
      }
    }
"#;

#[test]
fn a_structure_colliding_with_a_yang_data_container_is_a_schema_error() {
    // Both would be addressed as "/foo:doc" by the module's .sid file.
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          import ietf-yang-structure-ext {
            prefix sx;
          }

          rc:yang-data doc-template {
            container doc {
            }
          }
          sx:structure doc {
            leaf x {
              type string;
            }
          }
        }
    "#;
    let err = build(
        &[STRUCTURE_EXT.to_string(), src.to_string()],
        &[],
        &[],
        &[],
        &[],
    )
    .err()
    .expect("a structure colliding with a yang-data container must fail");
    assert!(err.0.contains("\"doc\""), "{}", err.0);
    assert!(err.0.contains("collides"), "{}", err.0);
}

#[test]
fn a_yang_data_container_colliding_with_a_structure_is_a_schema_error() {
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          import ietf-yang-structure-ext {
            prefix sx;
          }

          sx:structure doc {
            leaf x {
              type string;
            }
          }
          rc:yang-data doc-template {
            container doc {
            }
          }
        }
    "#;
    let err = build(
        &[STRUCTURE_EXT.to_string(), src.to_string()],
        &[],
        &[],
        &[],
        &[],
    )
    .err()
    .expect("a yang-data container colliding with a structure must fail");
    assert!(err.0.contains("\"doc-template\""), "{}", err.0);
    assert!(err.0.contains("collides"), "{}", err.0);
}

#[test]
fn a_structures_root_carries_its_own_status_and_description() {
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          import ietf-yang-structure-ext {
            prefix sx;
          }

          sx:structure old-doc {
            status deprecated;
            description "An old document format.";
            leaf name {
              type string;
            }
          }

          sx:structure doc {
            leaf name {
              type string;
            }
          }
        }
    "#;
    let schema = build(
        &[STRUCTURE_EXT.to_string(), src.to_string()],
        &[],
        &[],
        &[],
        &[],
    )
    .unwrap();
    let mut names: Vec<String> = schema.structure_roots().map(|r| r.name.clone()).collect();
    names.sort();
    assert_eq!(names, vec!["foo:doc", "foo:old-doc"]);
    let old = schema
        .structure_roots()
        .find(|r| r.name == "foo:old-doc")
        .unwrap();
    assert_eq!(old.status, Some(Status::Deprecated));
    assert_eq!(old.description.as_deref(), Some("An old document format."));
    // RFC 7950 7.21.2: status defaults to "current" when unstated.
    let doc = schema
        .structure_roots()
        .find(|r| r.name == "foo:doc")
        .unwrap();
    assert_eq!(doc.status, Some(Status::Current));
    assert_eq!(doc.description, None);
}

#[test]
fn a_structure_that_isnt_a_top_level_statement_is_a_schema_error() {
    // RFC 8791: "This extension is only valid as a top-level statement,
    // i.e., given as a substatement to 'module' or 'submodule'."
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          import ietf-yang-structure-ext {
            prefix sx;
          }

          container top {
            sx:structure doc {
              leaf name {
                type string;
              }
            }
          }
        }
    "#;
    let err = build(
        &[STRUCTURE_EXT.to_string(), src.to_string()],
        &[],
        &[],
        &[],
        &[],
    )
    .err()
    .unwrap();
    assert!(err.0.contains("\"doc\""), "{}", err.0);
    assert!(err.0.contains("top-level"), "{}", err.0);
}

#[test]
fn a_structure_defined_twice_in_one_module_is_a_schema_error() {
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          import ietf-yang-structure-ext {
            prefix sx;
          }

          sx:structure doc {
            leaf name {
              type string;
            }
          }

          sx:structure doc {
            leaf other {
              type string;
            }
          }
        }
    "#;
    let err = build(
        &[STRUCTURE_EXT.to_string(), src.to_string()],
        &[],
        &[],
        &[],
        &[],
    )
    .err()
    .unwrap();
    assert!(err.0.contains("\"doc\""), "{}", err.0);
    assert!(err.0.contains("more than once"), "{}", err.0);
}

#[test]
fn a_structure_colliding_with_a_top_level_data_node_is_a_schema_error() {
    // The structure's name and the module's top-level data nodes share
    // one identifier namespace (libyang rejects this collision too).
    // The container is declared after the structure, so the check has
    // to see the whole module, not just what came before.
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          import ietf-yang-structure-ext {
            prefix sx;
          }

          sx:structure doc {
            leaf name {
              type string;
            }
          }

          container doc {
            leaf other {
              type string;
            }
          }
        }
    "#;
    let err = build(
        &[STRUCTURE_EXT.to_string(), src.to_string()],
        &[],
        &[],
        &[],
        &[],
    )
    .err()
    .unwrap();
    assert!(err.0.contains("\"doc\""), "{}", err.0);
    assert!(err.0.contains("collides"), "{}", err.0);
}

#[test]
fn same_named_structures_in_different_modules_are_independent() {
    // Both checks are per module: "doc" in foo and "doc" in bar are
    // different structures, and bar's top-level "doc" container doesn't
    // collide with foo's structure.
    let foo = r#"
        module foo {
          namespace "urn:foo";
          prefix f;

          import ietf-yang-structure-ext {
            prefix sx;
          }

          sx:structure doc {
            leaf name {
              type string;
            }
          }
        }
    "#;
    let bar = r#"
        module bar {
          namespace "urn:bar";
          prefix b;

          import ietf-yang-structure-ext {
            prefix sx;
          }

          sx:structure doc {
            leaf other {
              type string;
            }
          }

          container top {
            leaf x {
              type string;
            }
          }
        }
    "#;
    let baz = r#"
        module baz {
          namespace "urn:baz";
          prefix z;

          container doc {
            leaf y {
              type string;
            }
          }
        }
    "#;
    let schema = build(
        &[
            STRUCTURE_EXT.to_string(),
            foo.to_string(),
            bar.to_string(),
            baz.to_string(),
        ],
        &[],
        &[],
        &[],
        &[],
    )
    .unwrap();
    assert!(schema
        .structure_roots()
        .find(|r| r.name == "foo:doc")
        .unwrap()
        .find_child("name")
        .is_some());
    assert!(schema
        .structure_roots()
        .find(|r| r.name == "bar:doc")
        .unwrap()
        .find_child("other")
        .is_some());
    assert!(schema.node_by_schema_path("/baz:doc").is_some());
}
