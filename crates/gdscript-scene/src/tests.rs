//! M0 unit tests — the header/ext/node matrix, the load-bearing value-lexer/skipper cases
//! (C1–C12 from `PHASE-4-M0-PLAYBOOK.md`), the tree walks, and the never-panic degrade paths.

use crate::{NodeIdx, SceneKind, SceneProblem, parse_scene};

/// The canonical must-pass scene (the real `ReactiveUI-Gadot/examples/main.tscn`).
const MAIN_TSCN: &str = "[gd_scene load_steps=2 format=3]\n\
\n\
[ext_resource type=\"Script\" path=\"res://examples/app.gd\" id=\"1_app\"]\n\
\n\
[node name=\"Main\" type=\"Control\"]\n\
layout_mode = 3\n\
anchors_preset = 15\n\
script = ExtResource(\"1_app\")\n";

#[test]
fn parses_the_target_main_tscn() {
    let m = parse_scene(MAIN_TSCN);
    assert_eq!(m.kind, SceneKind::Scene);
    assert_eq!(m.format, Some(3));
    assert!(m.problems.is_empty(), "{:?}", m.problems);
    // one ext_resource (the script), one node (the root).
    assert_eq!(m.ext_resources.len(), 1);
    assert_eq!(m.nodes.len(), 1);
    let root = m.root.expect("a root");
    let n = m.node(root).unwrap();
    assert_eq!(n.name, "Main");
    assert_eq!(n.decl_type.as_deref(), Some("Control"));
    assert!(n.parent_path.is_none(), "root has no parent");
    // the body `script = ExtResource("1_app")` resolved to the ext id.
    assert_eq!(n.script.as_ref().map(|e| e.0.as_str()), Some("1_app"));
    // association: the script path maps back to the root node.
    assert_eq!(m.node_with_script("res://examples/app.gd"), Some(root));
}

#[test]
fn header_matrix() {
    // uid present + script_class; gd_resource kind.
    let s = parse_scene("[gd_scene format=3 uid=\"uid://abc\" script_class=\"Foo\"]\n");
    assert_eq!(s.kind, SceneKind::Scene);
    assert_eq!(s.uid.as_deref(), Some("uid://abc"));
    assert_eq!(s.script_class.as_deref(), Some("Foo"));

    let r = parse_scene("[gd_resource type=\"Resource\" script_class=\"Dialogue\" format=3]\n");
    assert_eq!(r.kind, SceneKind::Resource);
    assert_eq!(r.resource_type.as_deref(), Some("Resource"));
    assert_eq!(r.script_class.as_deref(), Some("Dialogue"));

    // load_steps present but its value is irrelevant; no uid.
    let l = parse_scene("[gd_scene load_steps=99 format=3]\n");
    assert_eq!(l.format, Some(3));
    assert!(l.uid.is_none());
}

#[test]
fn ext_resource_matrix() {
    // quoted string id, bare-int id (3.x), Script type with uid, and a missing-field case.
    let m = parse_scene(
        "[gd_scene format=3]\n\
         [ext_resource type=\"Script\" uid=\"uid://x\" path=\"res://a.gd\" id=\"1_app\"]\n\
         [ext_resource type=\"PackedScene\" path=\"res://b.tscn\" id=1]\n\
         [ext_resource type=\"Texture2D\" id=\"3\"]\n",
    );
    let s = m.ext_resources.get(&crate::ExtId("1_app".into())).unwrap();
    assert_eq!(s.res_type, "Script");
    assert_eq!(s.path.as_deref(), Some("res://a.gd"));
    assert_eq!(s.uid.as_deref(), Some("uid://x"));
    // bare-int id normalized to the string "1".
    assert!(m.ext_resources.contains_key(&crate::ExtId("1".into())));
    // the Texture2D ext_resource is missing `path` → a MissingExtField problem (but still recorded).
    assert!(m.ext_resources.contains_key(&crate::ExtId("3".into())));
    assert!(
        m.problems
            .iter()
            .any(|p| matches!(p, SceneProblem::MissingExtField { .. }))
    );
}

#[test]
fn node_tree_paths_and_children() {
    let m = parse_scene(
        "[gd_scene format=3]\n\
         [node name=\"Root\" type=\"Control\"]\n\
         [node name=\"Panel\" type=\"Panel\" parent=\".\"]\n\
         [node name=\"VBox\" type=\"VBoxContainer\" parent=\"Panel\"]\n\
         [node name=\"StartButton\" type=\"Button\" parent=\"Panel/VBox\"]\n",
    );
    assert!(m.problems.is_empty(), "{:?}", m.problems);
    assert_eq!(m.nodes.len(), 4);
    let root = m.root.unwrap();

    // by_path / resolve_path from the root (root name excluded).
    let btn = m
        .resolve_path("Panel/VBox/StartButton")
        .expect("button by path");
    assert_eq!(m.node(btn).unwrap().name, "StartButton");
    assert_eq!(m.node(btn).unwrap().decl_type.as_deref(), Some("Button"));
    assert_eq!(m.by_path.get("Panel/VBox/StartButton").copied(), Some(btn));
    assert!(m.resolve_path("Panel/Nope").is_none());

    // resolve_path_from a non-root base (a script could attach to Panel).
    let panel = m.resolve_path("Panel").unwrap();
    assert_eq!(m.resolve_path_from(panel, "VBox/StartButton"), Some(btn));
    assert_eq!(m.resolve_path_from(panel, "."), Some(panel)); // self

    // children_of.
    let root_children: Vec<_> = m
        .children_of(Some(root))
        .map(|(_, n)| n.name.as_str())
        .collect();
    assert_eq!(root_children, vec!["Panel"]);
    let none_is_root: Vec<_> = m.children_of(None).map(|(_, n)| n.name.as_str()).collect();
    assert_eq!(none_is_root, vec!["Panel"]); // None ⇒ root's children

    // out-of-slice paths degrade to None (not an error).
    assert!(m.resolve_path("/root/Panel").is_none());
    assert!(m.resolve_path_from(panel, "../Root").is_none());
}

#[test]
fn instanced_child_and_inherited_root() {
    // An instanced child (no type=, instance=) and an inherited-scene root (no parent=, instance=).
    let child = parse_scene(
        "[gd_scene format=3]\n\
         [ext_resource type=\"PackedScene\" path=\"res://player.tscn\" id=\"1\"]\n\
         [node name=\"Root\" type=\"Node\"]\n\
         [node name=\"Player\" parent=\".\" instance=ExtResource(\"1\")]\n",
    );
    let player = child.resolve_path("Player").unwrap();
    let pn = child.node(player).unwrap();
    assert!(pn.decl_type.is_none(), "instanced node has no type=");
    assert_eq!(pn.instance.as_ref().map(|e| e.0.as_str()), Some("1"));
    assert!(!pn.instance_is_inherited_root);

    let inherited = parse_scene(
        "[gd_scene format=3]\n\
         [ext_resource type=\"PackedScene\" path=\"res://base.tscn\" id=\"1\"]\n\
         [node name=\"Derived\" instance=ExtResource(\"1\")]\n",
    );
    let r = inherited.root.unwrap();
    assert!(
        inherited.node(r).unwrap().instance_is_inherited_root,
        "root + instance= ⇒ inherited"
    );
}

#[test]
fn unique_name_in_owner_is_a_body_bool_not_the_header_unique_id() {
    // C3: `unique_name_in_owner` is a BODY property; `unique_id` is an unrelated header int.
    let m = parse_scene(
        "[gd_scene format=3]\n\
         [node name=\"Root\" type=\"Control\"]\n\
         [node name=\"Tabs\" type=\"TabContainer\" parent=\".\" unique_id=12345]\n\
         unique_name_in_owner = true\n",
    );
    assert!(m.problems.is_empty(), "{:?}", m.problems);
    let tabs = m.resolve_unique("Tabs").expect("%Tabs resolves");
    assert!(m.node(tabs).unwrap().unique_name_in_owner);
    assert_eq!(m.node(tabs).unwrap().name, "Tabs");
}

#[test]
fn header_value_lexer_tolerates_unquoted_ints_arrays_constructors() {
    // C1/C2: bare int `unique_id`, bracket-array `groups`, constructor `node_paths`, quoted `index`
    // — all dropped cleanly; name/type still correct.
    let m = parse_scene(
        "[gd_scene format=3]\n\
         [node name=\"N\" type=\"Button\" unique_id=1975992027 groups=[\"mobs\",\"ui\"] \
          node_paths=PackedStringArray(\"p\") index=\"0\"]\n",
    );
    assert!(
        m.problems
            .iter()
            .all(|p| !matches!(p, SceneProblem::MalformedHeader { .. })),
        "header must lex cleanly: {:?}",
        m.problems
    );
    let n = m.node(m.root.unwrap()).unwrap();
    assert_eq!(n.name, "N");
    assert_eq!(n.decl_type.as_deref(), Some("Button"));
}

#[test]
fn value_skipper_handles_multiline_color_and_embedded_newline() {
    // C11/C12: a `#`-color inside an array (not a comment), a multiline dict, and a quoted string
    // with a LITERAL newline — none of which may swallow the following real property.
    let m = parse_scene(
        "[gd_scene format=3]\n\
         [node name=\"L\" type=\"Label\"]\n\
         colors = [#ff0000, #00ff00]\n\
         data = {\n\
         \t\"a\": 1,\n\
         \t\"b\": [1, 2],\n\
         }\n\
         text = \"two\nlines\"\n\
         unique_name_in_owner = true\n",
    );
    assert!(m.problems.is_empty(), "{:?}", m.problems);
    // If any skip over-ran, the trailing `unique_name_in_owner = true` would have been missed.
    assert!(
        m.node(m.root.unwrap()).unwrap().unique_name_in_owner,
        "skipper consumed too much"
    );
}

#[test]
fn connection_and_tres_and_editable_are_recognized_not_errors() {
    let scene = parse_scene(
        "[gd_scene format=3]\n\
         [node name=\"Root\" type=\"Node\"]\n\
         [node name=\"Player\" type=\"Node\" parent=\".\"]\n\
         [connection signal=\"hit\" from=\"Player\" to=\".\" method=\"game_over\"]\n",
    );
    assert!(
        scene
            .problems
            .iter()
            .all(|p| !matches!(p, SceneProblem::UnknownTag { .. })),
        "connection must be recognized: {:?}",
        scene.problems
    );

    let tres = parse_scene(
        "[gd_resource type=\"Animation\" format=3]\n\
         [resource]\n\
         length = 1.5\n\
         tracks/0/type = \"value\"\n",
    );
    assert_eq!(tres.kind, SceneKind::Resource);
    assert_eq!(tres.resource_type.as_deref(), Some("Animation"));
    assert!(tres.problems.is_empty(), "{:?}", tres.problems);
}

#[test]
fn degrade_binary_and_unknown_tag() {
    let bin = parse_scene("RSRC\u{1}\u{2}\u{3}binary junk");
    assert!(bin.problems.contains(&SceneProblem::BinaryResource));
    assert!(bin.nodes.is_empty());

    let unknown = parse_scene(
        "[gd_scene format=3]\n\
         [weird_tag foo=\"bar\"]\n\
         baz = 1\n\
         [node name=\"Root\" type=\"Node\"]\n",
    );
    assert!(
        unknown
            .problems
            .iter()
            .any(|p| matches!(p, SceneProblem::UnknownTag { .. }))
    );
    // parsing continued past the unknown section: the real node is still there.
    assert_eq!(unknown.nodes.len(), 1);
}

#[test]
fn degrade_multiple_roots_no_root_and_dangling_parent() {
    let multi = parse_scene(
        "[gd_scene format=3]\n[node name=\"A\" type=\"Node\"]\n[node name=\"B\" type=\"Node\"]\n",
    );
    assert!(
        multi
            .problems
            .iter()
            .any(|p| matches!(p, SceneProblem::MultipleRoots { .. }))
    );
    assert_eq!(multi.root, Some(NodeIdx(0))); // first parent-less node wins

    let dangling = parse_scene(
        "[gd_scene format=3]\n\
         [node name=\"Root\" type=\"Node\"]\n\
         [node name=\"Lost\" type=\"Node\" parent=\"Ghost/Path\"]\n",
    );
    assert!(
        dangling
            .problems
            .iter()
            .any(|p| matches!(p, SceneProblem::DanglingParent { .. }))
    );
    // the lost node is recorded but not navigable.
    assert!(dangling.resolve_path("Ghost/Path/Lost").is_none());
}

#[test]
fn inline_subresource_script_is_not_a_dangling_ext_resource() {
    // `script = SubResource("…")` is an INLINE script, not an external attachment — it must not be
    // validated against the ext-resource table (no false UnknownExtResource), and M0 records no
    // external script for it (M1 types the node by its declared `type=`).
    let m = parse_scene(
        "[gd_scene format=3]\n\
         [sub_resource type=\"GDScript\" id=\"GDScript_x\"]\n\
         script/source = \"extends Node\"\n\
         [node name=\"Root\" type=\"Node\"]\n\
         script = SubResource(\"GDScript_x\")\n",
    );
    assert!(
        m.problems
            .iter()
            .all(|p| !matches!(p, SceneProblem::UnknownExtResource { .. })),
        "inline SubResource script must not be a dangling ext-resource: {:?}",
        m.problems
    );
    assert!(m.node(m.root.unwrap()).unwrap().script.is_none());
}

#[test]
fn parent_into_instanced_subscene_is_not_dangling_but_a_real_typo_is() {
    // A node parented THROUGH an instanced node (intermediate nodes live in the sub-scene we don't
    // recurse into) is an override — not a dangling parent.
    let ok = parse_scene(
        "[gd_scene format=3]\n\
         [ext_resource type=\"PackedScene\" path=\"res://bot.tscn\" id=\"1\"]\n\
         [node name=\"Root\" type=\"Node3D\"]\n\
         [node name=\"Bot\" parent=\".\" instance=ExtResource(\"1\")]\n\
         [node name=\"Extra\" type=\"Node3D\" parent=\"Bot/Armature/Skeleton3D\"]\n",
    );
    assert!(
        ok.problems
            .iter()
            .all(|p| !matches!(p, SceneProblem::DanglingParent { .. })),
        "an override into an instanced sub-scene must not be dangling: {:?}",
        ok.problems
    );

    // A genuine typo (parent of a non-instanced node) still flags.
    let typo = parse_scene(
        "[gd_scene format=3]\n\
         [node name=\"Root\" type=\"Control\"]\n\
         [node name=\"Panel\" type=\"Panel\" parent=\".\"]\n\
         [node name=\"X\" type=\"Label\" parent=\"Panl\"]\n",
    );
    assert!(
        typo.problems
            .iter()
            .any(|p| matches!(p, SceneProblem::DanglingParent { .. })),
        "a real typo'd parent must still flag: {:?}",
        typo.problems
    );
}

#[test]
fn override_children_under_an_inherited_root_are_not_dangling() {
    // The websocket_chat pattern: an INHERITED-scene root whose override children reference base
    // nodes (`Connect`) not redeclared here. The resolved prefix isn't itself an instance, but it
    // descends from the inherited root — so the missing tail is a base-scene node, not a dangling.
    let m = parse_scene(
        "[gd_scene format=3]\n\
         [ext_resource type=\"PackedScene\" path=\"res://base.tscn\" id=\"1\"]\n\
         [node name=\"Client\" instance=ExtResource(\"1\")]\n\
         [node name=\"Panel\" parent=\".\"]\n\
         [node name=\"VBoxContainer\" parent=\"Panel\"]\n\
         [node name=\"Port\" type=\"SpinBox\" parent=\"Panel/VBoxContainer/Connect\"]\n",
    );
    assert!(
        m.problems
            .iter()
            .all(|p| !matches!(p, SceneProblem::DanglingParent { .. })),
        "override under an inherited root must not be dangling: {:?}",
        m.problems
    );
}

#[test]
fn escape_parent_paths_degrade_silently_not_dangling() {
    // `..` / absolute / leading-`/` parent paths are out of the slice — silently unresolved, NOT a
    // dangling parent (Playbook §5/§7; M1 degrades them to `Node`). Found by the M0 bug hunt.
    for parent in ["../Sibling", "/root/R", "/R", "A/../B"] {
        let src = format!(
            "[gd_scene format=3]\n\
             [node name=\"R\" type=\"Node\"]\n\
             [node name=\"A\" type=\"Node\" parent=\".\"]\n\
             [node name=\"B\" type=\"Node\" parent=\"{parent}\"]\n"
        );
        let m = parse_scene(&src);
        assert!(
            m.problems
                .iter()
                .all(|p| !matches!(p, SceneProblem::DanglingParent { .. })),
            "parent={parent:?} must not dangle: {:?}",
            m.problems
        );
    }
    // A genuine in-scene typo still flags (regression guard).
    let typo = parse_scene(
        "[gd_scene format=3]\n[node name=\"R\" type=\"Node\"]\n[node name=\"B\" type=\"Node\" parent=\"Nope\"]\n",
    );
    assert!(
        typo.problems
            .iter()
            .any(|p| matches!(p, SceneProblem::DanglingParent { .. }))
    );
}

#[test]
fn duplicate_sibling_first_wins_for_path_resolution() {
    // by_path / resolve_path return the FIRST same-named sibling (matching unique_nodes' first-wins);
    // children_of still lists both. Found by the M0 bug hunt.
    let m = parse_scene(
        "[gd_scene format=3]\n\
         [node name=\"R\" type=\"Node\"]\n\
         [node name=\"Dup\" type=\"Label\" parent=\".\"]\n\
         [node name=\"Dup\" type=\"Button\" parent=\".\"]\n",
    );
    let dup = m.resolve_path("Dup").unwrap();
    assert_eq!(
        m.node(dup).unwrap().decl_type.as_deref(),
        Some("Label"),
        "first sibling wins"
    );
    assert_eq!(
        m.children_of(m.root).count(),
        2,
        "children_of lists both siblings"
    );
}

#[test]
fn never_panics_on_garbage() {
    for g in [
        "",
        "   \n\n  ",
        "[",
        "[node",
        "[node name=",
        "[node name=\"unterminated",
        "garbage not a scene at all",
        "[gd_scene format=3]\n[node name=\"a\" parent=\"a\"]\n", // self-parent (cyclic-ish)
        "[node name=\"x\"]\n}}}]]])))\n",
        "\u{feff}[gd_scene format=3]\n", // leading BOM
    ] {
        let _ = parse_scene(g); // must not panic
    }
}

#[test]
fn captures_connections_with_inner_identifier_spans() {
    let src = "[gd_scene format=3]\n\
[node name=\"Main\" type=\"Control\"]\n\
[node name=\"StartButton\" type=\"Button\" parent=\".\"]\n\
[connection signal=\"pressed\" from=\"StartButton\" to=\".\" method=\"_on_start_pressed\"]\n";
    let m = parse_scene(src);
    assert!(m.problems.is_empty(), "{:?}", m.problems);
    assert_eq!(m.connections.len(), 1);
    let c = &m.connections[0];
    assert_eq!(c.signal, "pressed");
    assert_eq!(c.from, "StartButton");
    assert_eq!(c.to, ".");
    assert_eq!(c.method, "_on_start_pressed");
    // Each span is the inner identifier (quotes excluded) — exactly what a rename rewrites.
    let at = |r: gdscript_base::TextRange| &src[r.start as usize..r.end as usize];
    assert_eq!(at(c.signal_span), "pressed");
    assert_eq!(at(c.from_span), "StartButton");
    assert_eq!(at(c.to_span), ".");
    assert_eq!(at(c.method_span), "_on_start_pressed");
}
