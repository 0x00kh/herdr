use super::super::space_groups::{self, SpaceRow};
use super::*;

fn group_state(count: usize) -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let mut projected = snapshot();
    projected.workspaces = (1..=count)
        .map(|number| {
            let mut workspace = projected.workspaces[0].clone();
            workspace.workspace_id = format!("ws_{number}");
            workspace.label = format!("project-{number}");
            workspace.new_workspace_cwd = format!("/repo-{number}");
            workspace.focused = number == 1;
            workspace
        })
        .collect();
    let tab = projected.tabs[0].clone();
    let pane = projected.panes[0].clone();
    projected.tabs.clear();
    projected.panes.clear();
    for (index, workspace) in projected.workspaces.iter_mut().enumerate() {
        workspace.active_tab_id = format!("tab_{}", index + 1);
        let mut tab = tab.clone();
        tab.tab_id.clone_from(&workspace.active_tab_id);
        tab.workspace_id.clone_from(&workspace.workspace_id);
        tab.focused = workspace.focused;
        let mut pane = pane.clone();
        pane.pane_id = format!("pane_{}", index + 1);
        pane.workspace_id.clone_from(&workspace.workspace_id);
        pane.tab_id.clone_from(&workspace.active_tab_id);
        pane.cwd = Some(workspace.new_workspace_cwd.clone());
        pane.focused = workspace.focused;
        projected.tabs.push(tab);
        projected.panes.push(pane);
    }
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state
}

fn create(state: &mut ClientShellState, endpoint: &ClientEndpointId, workspace: &str, name: &str) {
    state.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
        title: "new group",
        input: name.into(),
        target: ClientRenameTarget::SpaceGroup {
            endpoint_id: endpoint.clone(),
            old_name: None,
            workspace_id: Some(workspace.into()),
        },
    }));
    let mut outcome = ClientShellInput::default();
    state.save_rename_overlay(&mut outcome);
    assert!(outcome.actions.is_empty());
    assert!(outcome.requests.is_empty());
    assert!(state.overlay.is_none());
}

fn projected(state: &ClientShellState) -> Vec<SpaceRow> {
    space_groups::rows(
        state.snapshot.as_deref().unwrap(),
        &state.collapsed_groups,
        state.space_groups.get(&state.active_endpoint_id),
    )
}

fn order(state: &ClientShellState) -> Vec<String> {
    projected(state)
        .iter()
        .filter_map(|row| row.workspace())
        .map(|entry| {
            state.snapshot.as_ref().unwrap().workspaces[entry.index]
                .workspace_id
                .clone()
        })
        .collect()
}

fn activate_menu(
    state: &mut ClientShellState,
    action: ClientContextMenuAction,
) -> ClientShellInput {
    let Some(ClientShellOverlay::ContextMenu(menu)) = &state.overlay else {
        panic!("context menu");
    };
    let index = menu
        .items()
        .iter()
        .position(|item| item.action == action)
        .unwrap();
    let mut outcome = ClientShellInput::default();
    state.activate_context_menu_item(index, &mut outcome);
    outcome
}

#[test]
fn manual_space_groups_ungrouped_separator_is_muted_and_not_interactive() {
    let config = ClientShellConfig::from_config(&Config::default());
    for width in [0, 1, 2, 3, 4, 6, 28] {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 40, 3));
        let mut hits = ShellHitMap::default();
        let rect = Rect::new(2, 1, width, 1);
        space_groups::render_header(
            &mut buffer,
            rect,
            &SpaceRow::Header {
                group: None,
                count: 3,
                status: AgentStatus::Blocked,
            },
            &[],
            &ClientEndpointId::Local,
            &config,
            &mut hits,
        );
        for x in 0..40 {
            let cell = &buffer[(x, 1)];
            if x >= rect.x + 3 && x < rect.right().saturating_sub(1) {
                assert_eq!(cell.symbol(), "\u{2500}");
                assert_eq!(cell.fg, config.palette.surface_dim);
                assert!(!cell.modifier.contains(Modifier::BOLD));
            } else {
                assert_eq!(cell.symbol(), " ");
            }
        }
        assert!(hits.space_groups.is_empty());
        assert!(hits.workspaces.is_empty());
    }
}

#[test]
fn manual_space_groups_menu_create_move_rename_delete_is_client_only() {
    let mut state = group_state(3);
    let original = serde_json::to_value(state.snapshot.as_deref().unwrap()).unwrap();
    state.open_workspace_context_menu("ws_3".into(), 2, 3);
    assert!(
        activate_menu(&mut state, ClientContextMenuAction::MoveToSpaceGroup)
            .actions
            .is_empty()
    );
    activate_menu(&mut state, ClientContextMenuAction::NewSpaceGroup);
    state.insert_overlay_text("Work");
    let mut outcome = ClientShellInput::default();
    state.save_rename_overlay(&mut outcome);
    assert_eq!(order(&state), ["ws_3", "ws_1", "ws_2"]);
    state.open_move_to_group("ws_2".into(), 2, 3);
    activate_menu(&mut state, ClientContextMenuAction::AssignSpaceGroup(0));
    assert_eq!(order(&state), ["ws_2", "ws_3", "ws_1"]);
    state.compose(106, 40).unwrap();
    let rect = state.hits.space_groups[0].0;
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column: rect.x + 2,
        row: rect.y,
        modifiers: KeyModifiers::empty(),
    })]);
    activate_menu(&mut state, ClientContextMenuAction::Rename);
    if let Some(ClientShellOverlay::Rename(rename)) = &mut state.overlay {
        rename.input = "Projects".into();
    }
    state.save_rename_overlay(&mut outcome);
    assert_eq!(
        state.space_groups.get(&ClientEndpointId::Local)[0].name,
        "Projects"
    );
    state.delete_space_group(&ClientEndpointId::Local, "Projects", &mut outcome);
    assert_eq!(order(&state), ["ws_1", "ws_2", "ws_3"]);
    assert!(outcome.actions.is_empty());
    assert!(outcome.requests.is_empty());
    assert_eq!(
        serde_json::to_value(state.snapshot.as_deref().unwrap()).unwrap(),
        original
    );
}

#[test]
fn manual_space_groups_collapse_keeps_focus_and_hidden_attention() {
    let mut state = group_state(3);
    create(&mut state, &ClientEndpointId::Local, "ws_1", "Work");
    state.assign_space_group(
        &ClientEndpointId::Local,
        "ws_2",
        Some("Work"),
        &mut ClientShellInput::default(),
    );
    let mut snapshot = state.snapshot.as_deref().unwrap().clone();
    snapshot.workspaces[1].agent_status = AgentStatus::Blocked;
    state.set_snapshot(Box::new(snapshot));
    state.compose(106, 40).unwrap();
    let rect = state.hits.space_groups[0].0;
    let outcome = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x + 2,
        row: rect.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(outcome.actions.is_empty());
    assert!(matches!(
        projected(&state)[0],
        SpaceRow::Header {
            count: 2,
            status: AgentStatus::Blocked,
            ..
        }
    ));
    assert_eq!(order(&state), ["ws_1", "ws_3"]);
    state.compose(106, 40).unwrap();
    assert_eq!(state.hits.workspaces.len(), 2);
}

#[test]
fn manual_space_groups_worktree_family_moves_and_new_children_inherit() {
    let mut state = group_state(3);
    let mut snapshot = state.snapshot.as_deref().unwrap().clone();
    for (index, linked) in [(0, false), (2, true)] {
        snapshot.workspaces[index].worktree = Some(ClientShellWorktree {
            key: "/repo".into(),
            label: "repo".into(),
            is_linked_worktree: linked,
        });
    }
    state.set_snapshot(Box::new(snapshot.clone()));
    create(&mut state, &ClientEndpointId::Local, "ws_3", "Work");
    assert_eq!(order(&state), ["ws_1", "ws_3", "ws_2"]);
    let mut child = snapshot.workspaces[2].clone();
    child.workspace_id = "ws_4".into();
    snapshot.workspaces.push(child);
    state.set_snapshot(Box::new(snapshot));
    assert_eq!(order(&state), ["ws_1", "ws_3", "ws_4", "ws_2"]);
    assert!(projected(&state)[2].workspace().unwrap().indented);
    let mut focused_child = state.snapshot.as_deref().unwrap().clone();
    focused_child.workspaces[0].focused = false;
    focused_child.workspaces[2].focused = true;
    focused_child.focused_workspace_id = Some("ws_3".into());
    state.set_snapshot(Box::new(focused_child));
    state.toggle_space_group(
        &ClientEndpointId::Local,
        "Work",
        &mut ClientShellInput::default(),
    );
    assert_eq!(order(&state), ["ws_1", "ws_3", "ws_2"]);
    state.toggle_space_group(
        &ClientEndpointId::Local,
        "Work",
        &mut ClientShellInput::default(),
    );
    state.collapsed_groups.insert("/repo".into());
    assert_eq!(order(&state), ["ws_1", "ws_3", "ws_2"]);
    state.assign_space_group(
        &ClientEndpointId::Local,
        "ws_4",
        None,
        &mut ClientShellInput::default(),
    );
    assert!(matches!(
        projected(&state)[0],
        SpaceRow::Header { count: 0, .. }
    ));
    assert!(state.collapsed_groups.contains("/repo"));
}

#[test]
fn manual_space_groups_preferences_restore_and_guard_reused_ids() {
    let root = std::env::temp_dir().join(format!("herdr-manual-groups-{}", std::process::id()));
    let path = root.join("preferences.json");
    let mut state = group_state(3);
    state.config.preferences_path = Some(path.clone());
    create(&mut state, &ClientEndpointId::Local, "ws_3", "Work");
    state.toggle_space_group(
        &ClientEndpointId::Local,
        "Work",
        &mut ClientShellInput::default(),
    );
    let mut snapshot = state.snapshot.as_deref().unwrap().clone();
    snapshot.panes[2].cwd = Some("/changed-cwd".into());
    state.set_snapshot(Box::new(snapshot.clone()));
    let mut restored = ClientShellState::new(
        ClientShellConfig::from_config(&Config::default()).with_preferences_path(path),
    );
    snapshot.boot_id = "restored-boot".into();
    snapshot.workspaces[2].new_workspace_cwd = "/new-terminal-policy".into();
    restored.set_snapshot(Box::new(snapshot.clone()));
    assert!(restored.space_groups.get(&ClientEndpointId::Local)[0].collapsed);
    assert!(matches!(
        projected(&restored)[0],
        SpaceRow::Header { count: 1, .. }
    ));
    snapshot.boot_id = "different-boot".into();
    snapshot.panes[2].cwd = Some("/unrelated-project".into());
    restored.set_snapshot(Box::new(snapshot));
    assert!(matches!(
        projected(&restored)[0],
        SpaceRow::Header { count: 0, .. }
    ));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn manual_space_groups_worktree_adoption_unifies_membership_without_hidden_assignments() {
    let mut state = group_state(2);
    create(&mut state, &ClientEndpointId::Local, "ws_1", "Parent");
    create(&mut state, &ClientEndpointId::Local, "ws_2", "Child");
    let mut snapshot = state.snapshot.as_deref().unwrap().clone();
    for (index, workspace) in snapshot.workspaces.iter_mut().enumerate() {
        workspace.worktree = Some(ClientShellWorktree {
            key: "/repo".into(),
            label: "repo".into(),
            is_linked_worktree: index == 1,
        });
    }
    state.set_snapshot(Box::new(snapshot));
    assert!(matches!(
        projected(&state)[0],
        SpaceRow::Header { count: 2, .. }
    ));
    state.delete_space_group(
        &ClientEndpointId::Local,
        "Parent",
        &mut ClientShellInput::default(),
    );
    let rows = projected(&state);
    assert!(matches!(rows[0], SpaceRow::Header { count: 0, .. }));
    assert!(matches!(
        rows[1],
        SpaceRow::Header {
            group: None,
            count: 2,
            ..
        }
    ));
}

#[test]
fn manual_space_groups_scrolled_menu_hover_preserves_clicked_group() {
    let mut state = group_state(1);
    for index in 0..30 {
        create(
            &mut state,
            &ClientEndpointId::Local,
            "ws_1",
            &format!("Group {index}"),
        );
    }
    state.open_move_to_group("ws_1".into(), 2, 3);
    state.compose(106, 20).unwrap();
    state.move_context_menu_selection(29);
    state.compose(106, 20).unwrap();
    let (rect, index) = state.hits.context_menu_rows[2];
    assert!(index > 2);
    let expected = if let Some(ClientShellOverlay::ContextMenu(menu)) = &state.overlay {
        menu.items()[index].label.to_string()
    } else {
        panic!("menu");
    };
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column: rect.x + 1,
        row: rect.y,
        modifiers: KeyModifiers::empty(),
    })]);
    state.compose(106, 20).unwrap();
    assert_eq!(state.hits.context_menu_rows[2], (rect, index));
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x + 1,
        row: rect.y,
        modifiers: KeyModifiers::empty(),
    })]);
    let rows = projected(&state);
    let populated = rows
        .iter()
        .find_map(|row| match row {
            SpaceRow::Header {
                group: Some(index),
                count: 1,
                ..
            } => Some(*index),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        state.space_groups.get(&ClientEndpointId::Local)[populated].name,
        expected
    );
}

#[test]
fn manual_space_groups_endpoints_with_identical_ids_are_isolated() {
    let mut state = group_state(1);
    let profile = SavedSshEndpoint {
        id: crate::client::endpoint::ProfileId::parse("0123456789abcdef0123456789abcdef").unwrap(),
        label: "Build".into(),
        target: "dev@build.example".into(),
        session: "agents".into(),
        enabled: true,
    };
    let remote = ClientEndpointId::Ssh(profile.id.clone());
    state.set_endpoint_catalog(&[profile]);
    state.set_endpoint_status(&remote, ClientEndpointStatus::Online);
    let mut remote_snapshot = snapshot();
    remote_snapshot.boot_id = "remote-boot".into();
    state.set_endpoint_snapshot(&remote, Box::new(remote_snapshot));
    create(&mut state, &ClientEndpointId::Local, "ws_1", "Local work");
    create(&mut state, &remote, "ws_1", "Remote work");
    state.compose(106, 40).unwrap();
    assert_eq!(state.hits.space_groups.len(), 2);
    state.pane_surface = None;
    state.compose(106, 40).unwrap();
    let rect = state.hits.space_groups[0].0;
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column: rect.x + 2,
        row: rect.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(
        state.overlay.is_none(),
        "unavailable surfaces cannot render a context menu"
    );
    state.delete_space_group(&remote, "Remote work", &mut ClientShellInput::default());
    assert_eq!(state.space_groups.get(&ClientEndpointId::Local).len(), 1);
    assert!(state.space_groups.get(&remote).is_empty());
}

#[test]
fn manual_space_groups_keyboard_order_and_header_inclusive_scrolling_agree() {
    let mut state = group_state(16);
    for number in (1..=16).rev() {
        create(
            &mut state,
            &ClientEndpointId::Local,
            &format!("ws_{number}"),
            &format!("Group {number}"),
        );
    }
    state.compose(106, 20).unwrap();
    state.handle_input_bytes(&[0x02]);
    state.handle_input_bytes(b"w");
    for _ in 0..15 {
        state.move_navigate_workspace(-1);
    }
    assert_eq!(
        state.navigate_workspace_id.as_ref().unwrap().workspace_id,
        "ws_16"
    );
    state.compose(106, 20).unwrap();
    assert!(state
        .hits
        .workspaces
        .iter()
        .any(|hit| hit.workspace_id == "ws_16"));
    for _ in 0..15 {
        state.move_navigate_workspace(1);
    }
    state.compose(106, 20).unwrap();
    assert!(state
        .hits
        .workspaces
        .iter()
        .any(|hit| hit.workspace_id == "ws_1"));
    assert_eq!(
        state.navigation_workspace_entries(state.snapshot.as_deref().unwrap())[0].index,
        15
    );
    assert!(
        !state.endpoint_workspace_is_draggable(&ClientWorkspacePress {
            endpoint_id: ClientEndpointId::Local,
            workspace_id: "ws_1".into(),
            start_column: 1,
            start_row: 1,
        })
    );
}

#[test]
fn manual_space_groups_blank_or_duplicate_names_stay_editable() {
    let mut state = group_state(2);
    create(&mut state, &ClientEndpointId::Local, "ws_1", "Work");
    for name in [" ", "Work"] {
        state.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
            title: "new group",
            input: name.into(),
            target: ClientRenameTarget::SpaceGroup {
                endpoint_id: ClientEndpointId::Local,
                old_name: None,
                workspace_id: Some("ws_2".into()),
            },
        }));
        state.save_rename_overlay(&mut ClientShellInput::default());
        assert!(matches!(state.overlay, Some(ClientShellOverlay::Rename(_))));
        assert_eq!(state.space_groups.get(&ClientEndpointId::Local).len(), 1);
    }
}

#[test]
fn manual_space_groups_compact_and_mobile_keep_all_workspaces_accessible() {
    let mut state = group_state(3);
    create(&mut state, &ClientEndpointId::Local, "ws_2", "Work");
    state.toggle_space_group(
        &ClientEndpointId::Local,
        "Work",
        &mut ClientShellInput::default(),
    );
    state.sidebar_collapsed = true;
    state.compose(106, 40).unwrap();
    assert!(state.hits.space_groups.is_empty());
    assert_eq!(state.hits.workspaces.len(), 3);
    state.compose(40, 24).unwrap();
    assert_eq!(
        state
            .navigation_workspace_entries(state.snapshot.as_deref().unwrap())
            .len(),
        3
    );
}

#[test]
#[ignore = "manual fixed-geometry space grouping composition profile"]
fn manual_space_groups_render_scale_profile() {
    for count in [1, 15] {
        for grouped in [false, true] {
            let mut state = group_state(count);
            if grouped {
                for number in 1..=count {
                    create(
                        &mut state,
                        &ClientEndpointId::Local,
                        &format!("ws_{number}"),
                        &format!("Group {number}"),
                    );
                }
            }
            for _ in 0..20 {
                std::hint::black_box(state.compose(106, 40).unwrap());
            }
            let start = std::time::Instant::now();
            for _ in 0..1000 {
                std::hint::black_box(state.compose(106, 40).unwrap());
            }
            eprintln!(
                "manual groups: {count} workspaces, grouped={grouped}, {:.1} us/frame",
                start.elapsed().as_secs_f64() * 1000.0
            );
        }
    }
}
