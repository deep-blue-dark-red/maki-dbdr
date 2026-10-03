use std::sync::Arc;

use maki_agent::AgentMode;
use maki_agent::tools::test_support::stub_ctx;
use maki_lua::PluginHost;
use serde_json::json;

fn exec(reg: &maki_agent::tools::ToolRegistry, name: &str, input: serde_json::Value) -> String {
    let entry = reg
        .get(name)
        .unwrap_or_else(|| panic!("tool {name} not registered"));
    let inv = entry.tool.parse(&input).expect("parse failed");
    let ctx = stub_ctx(&AgentMode::Build);
    smol::block_on(async { inv.execute(&ctx).await })
        .output
        .map(|out| match out {
            maki_agent::ToolOutput::Plain(s) => s.text,
            other => panic!("unexpected output: {other:?}"),
        })
        .unwrap_or_else(|e| panic!("{name} failed: {e}"))
}

/// The user-plugin flow end to end: the bundled create_plugin tool scaffolds
/// into a config dir, an init chunk requires the module, and the tool it
/// registers is callable without a rebuild.
#[test]
fn scaffolded_user_plugin_loads_and_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let config_dir = tmp.path().join(".maki");

    let reg = Arc::new(maki_agent::tools::ToolRegistry::new());
    let host = PluginHost::with_all_builtins(Arc::clone(&reg)).unwrap();

    let out = exec(
        &reg,
        "create_plugin",
        json!({ "name": "greet", "path": config_dir.to_str().unwrap(), "description": "Echo the query." }),
    );
    assert!(
        out.contains("require(\"greet\")"),
        "wiring steps missing: {out}"
    );
    assert!(
        out.contains("[permissions]"),
        "manifest step missing: {out}"
    );
    assert!(out.contains("/reload"), "reload step missing: {out}");

    let module = config_dir.join("lua").join("greet.lua");
    let source = std::fs::read_to_string(&module).unwrap();
    assert!(source.contains("name = \"greet\""));
    assert_eq!(
        std::fs::read_to_string(config_dir.join("plugin.toml")).unwrap(),
        "[permissions]\n"
    );

    // What ~/.maki/init.lua does after the agent wires it: require the module
    // from the config dir's lua/ tree.
    host.send_run_init_lua(
        "require(\"greet\")".to_owned(),
        "test_init".to_owned(),
        Some(config_dir.clone()),
    )
    .unwrap();

    assert_eq!(
        exec(&reg, "greet", json!({ "query": "hello" })),
        "hello",
        "scaffolded tool should be registered and runnable"
    );
}
