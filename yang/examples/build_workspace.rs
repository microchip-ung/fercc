use std::fs;
use std::path::Path;

fn main() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    let cfg_path = repo_root.join("support/scripts/gen-cc-nodes.yaml");
    let yang_dir = repo_root.join("docs/sw_refs/yang");

    let cfg_text = fs::read_to_string(&cfg_path).expect("read gen-cc-nodes.yaml");
    // Minimal ad-hoc extraction of the flat "sid-files:" list -- avoids
    // pulling in a YAML parser dependency just for this scratch example.
    let mut sid_files = Vec::new();
    let mut in_list = false;
    for line in cfg_text.lines() {
        if line.trim_start() == "sid-files:" || line == "sid-files:" {
            in_list = true;
            continue;
        }
        if in_list {
            if let Some(rest) = line.strip_prefix("- ") {
                sid_files.push(rest.trim().to_string());
            } else {
                break;
            }
        }
    }
    println!("{} sid-files listed", sid_files.len());

    let mut yang_sources = Vec::new();
    let mut sid_sources = Vec::new();
    for entry in &sid_files {
        let sid_path = yang_dir.join(entry);
        let yang_name = format!("{}.yang", entry.split('@').next().unwrap());
        let yang_path = yang_dir.join(&yang_name);
        sid_sources.push(fs::read_to_string(&sid_path).unwrap_or_else(|e| panic!("{}: {}", sid_path.display(), e)));
        yang_sources.push(fs::read_to_string(&yang_path).unwrap_or_else(|e| panic!("{}: {}", yang_path.display(), e)));
    }

    println!("building schema from {} modules...", yang_sources.len());
    match yang::schema::build(&yang_sources, &sid_sources) {
        Ok(schema) => {
            println!("OK: {} nodes, {} types, {} identities, {} SIDs indexed", schema.nodes.len(), schema.types.len(), schema.identities.len(), schema.sid_index.len());
            let root = schema.root;
            println!("root children (top-level data nodes): {}", schema.node(root).children.len());
            for &c in schema.node(root).children.iter().take(10) {
                println!("  {} {} sid={:?}", schema.node(c).kw, schema.node(c).name, schema.node(c).sid);
            }
        }
        Err(e) => {
            println!("FAILED: {e}");
            std::process::exit(1);
        }
    }
}
