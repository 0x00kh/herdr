use super::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct SpaceGroups {
    #[serde(flatten)]
    endpoints: BTreeMap<String, EndpointGroups>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct EndpointGroups {
    boot_id: String,
    groups: Vec<SpaceGroup>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct SpaceGroup {
    pub name: String,
    pub collapsed: bool,
    members: Vec<GroupMember>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct GroupMember {
    workspace_id: String,
    cwd: Option<String>,
    worktree_key: Option<String>,
}

fn endpoint_key(endpoint: &ClientEndpointId) -> &str {
    match endpoint {
        ClientEndpointId::Local => "local",
        ClientEndpointId::Ssh(id) => id.as_str(),
    }
}

fn pane_cwds(snapshot: &ClientShellSnapshot) -> HashMap<&str, Vec<&str>> {
    let mut cwds = HashMap::<&str, Vec<&str>>::new();
    for pane in &snapshot.panes {
        if let Some(cwd) = pane.cwd.as_deref() {
            cwds.entry(&pane.workspace_id).or_default().push(cwd);
        }
    }
    cwds
}

impl SpaceGroups {
    pub(super) fn get(&self, endpoint: &ClientEndpointId) -> &[SpaceGroup] {
        self.endpoints
            .get(endpoint_key(endpoint))
            .map_or(&[], |saved| saved.groups.as_slice())
    }

    fn get_mut(&mut self, endpoint: &ClientEndpointId) -> &mut Vec<SpaceGroup> {
        &mut self
            .endpoints
            .entry(endpoint_key(endpoint).into())
            .or_default()
            .groups
    }

    // IDs survive restore but can be reused after a server restart. Check the
    // saved fingerprint only across boots: cwd changes during a session are normal.
    pub(super) fn reconcile(
        &mut self,
        endpoint: &ClientEndpointId,
        snapshot: &ClientShellSnapshot,
    ) -> bool {
        let Some(saved) = self.endpoints.get_mut(endpoint_key(endpoint)) else {
            return false;
        };
        if saved.groups.is_empty() {
            return false;
        }
        let mut changed = saved.boot_id != snapshot.boot_id;
        let cwds = pane_cwds(snapshot);
        let workspaces = snapshot
            .workspaces
            .iter()
            .map(|workspace| (workspace.workspace_id.as_str(), workspace))
            .collect::<HashMap<_, _>>();
        for group in &mut saved.groups {
            group.members.retain_mut(|member| {
                let Some(workspace) = workspaces.get(member.workspace_id.as_str()) else {
                    changed = true;
                    return false;
                };
                let key = workspace.worktree.as_ref().map(|tree| tree.key.as_str());
                let paths = cwds.get(member.workspace_id.as_str());
                let cwd_matches = member
                    .cwd
                    .as_deref()
                    .is_none_or(|cwd| paths.is_none_or(|paths| paths.contains(&cwd)));
                let same_repo = key.is_some() && member.worktree_key.as_deref() == key;
                if saved.boot_id != snapshot.boot_id && !same_repo && !cwd_matches {
                    changed = true;
                    return false;
                }
                if !cwd_matches || member.cwd.is_none() {
                    if let Some(cwd) = paths.and_then(|paths| paths.first()) {
                        member.cwd = Some((*cwd).to_owned());
                        changed = true;
                    }
                }
                if member.worktree_key.as_deref() != key {
                    member.worktree_key = key.map(str::to_owned);
                    changed = true;
                }
                true
            });
        }
        let owners = saved
            .groups
            .iter()
            .enumerate()
            .flat_map(|(index, group)| {
                group
                    .members
                    .iter()
                    .map(move |member| (member.workspace_id.as_str(), index))
            })
            .collect::<HashMap<_, _>>();
        let mut families = HashMap::new();
        // An adopted checkout follows its parent's group; if only a child was
        // grouped, let it supply the family membership instead.
        for parent_only in [true, false] {
            for workspace in &snapshot.workspaces {
                let Some(tree) = &workspace.worktree else {
                    continue;
                };
                if parent_only && tree.is_linked_worktree {
                    continue;
                }
                if let Some(owner) = owners.get(workspace.workspace_id.as_str()) {
                    families.entry(tree.key.as_str()).or_insert(*owner);
                }
            }
        }
        let transfers = snapshot
            .workspaces
            .iter()
            .filter_map(|workspace| {
                let tree = workspace.worktree.as_ref()?;
                let owner = *families.get(tree.key.as_str())?;
                if owners.get(workspace.workspace_id.as_str()) == Some(&owner) {
                    return None;
                }
                Some((
                    owner,
                    GroupMember {
                        workspace_id: workspace.workspace_id.clone(),
                        cwd: cwds
                            .get(workspace.workspace_id.as_str())
                            .and_then(|paths| paths.first())
                            .map(|cwd| (*cwd).to_owned()),
                        worktree_key: Some(tree.key.clone()),
                    },
                ))
            })
            .collect::<Vec<_>>();
        changed |= !transfers.is_empty();
        if !transfers.is_empty() {
            let moved = transfers
                .iter()
                .map(|(_, member)| member.workspace_id.as_str())
                .collect::<HashSet<_>>();
            for group in &mut saved.groups {
                group
                    .members
                    .retain(|member| !moved.contains(member.workspace_id.as_str()));
            }
            for (index, member) in transfers {
                saved.groups[index].members.push(member);
            }
        }
        saved.boot_id.clone_from(&snapshot.boot_id);
        changed
    }

    fn assign(
        &mut self,
        endpoint: &ClientEndpointId,
        snapshot: &ClientShellSnapshot,
        workspace_id: &str,
        group_name: Option<&str>,
    ) {
        let Some(workspace) = snapshot
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == workspace_id)
        else {
            return;
        };
        let cwds = pane_cwds(snapshot);
        let key = workspace.worktree.as_ref().map(|tree| tree.key.as_str());
        let family = snapshot
            .workspaces
            .iter()
            .filter(|candidate| {
                candidate.workspace_id == workspace_id
                    || key.is_some_and(|key| {
                        candidate
                            .worktree
                            .as_ref()
                            .is_some_and(|tree| tree.key == key)
                    })
            })
            .collect::<Vec<_>>();
        let ids = family
            .iter()
            .map(|workspace| workspace.workspace_id.as_str())
            .collect::<HashSet<_>>();
        let saved = self
            .endpoints
            .entry(endpoint_key(endpoint).into())
            .or_default();
        if group_name.is_some_and(|name| !saved.groups.iter().any(|group| group.name == name)) {
            return;
        }
        saved.boot_id.clone_from(&snapshot.boot_id);
        for group in &mut saved.groups {
            group
                .members
                .retain(|member| !ids.contains(member.workspace_id.as_str()));
            if group_name == Some(group.name.as_str()) {
                group.members.extend(family.iter().map(|workspace| {
                    GroupMember {
                        workspace_id: workspace.workspace_id.clone(),
                        cwd: cwds
                            .get(workspace.workspace_id.as_str())
                            .and_then(|paths| paths.first())
                            .map(|cwd| (*cwd).to_owned()),
                        worktree_key: workspace.worktree.as_ref().map(|tree| tree.key.clone()),
                    }
                }));
            }
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum SpaceRow {
    Header {
        group: Option<usize>,
        count: usize,
        status: crate::api::schema::AgentStatus,
    },
    Workspace {
        entry: WorkspaceEntry,
        grouped: bool,
    },
}

impl SpaceRow {
    pub(super) fn workspace(&self) -> Option<&WorkspaceEntry> {
        match self {
            Self::Workspace { entry, .. } => Some(entry),
            Self::Header { .. } => None,
        }
    }
}

pub(super) fn rows(
    snapshot: &ClientShellSnapshot,
    collapsed: &HashSet<String>,
    groups: &[SpaceGroup],
) -> Vec<SpaceRow> {
    let entries = sidebar::workspace_entries(snapshot, collapsed);
    if groups.is_empty() {
        return entries
            .into_iter()
            .map(|entry| SpaceRow::Workspace {
                entry,
                grouped: false,
            })
            .collect();
    }
    let mut owners = HashMap::new();
    let mut families = HashMap::new();
    for (index, group) in groups.iter().enumerate() {
        for member in &group.members {
            owners.insert(member.workspace_id.as_str(), index);
            if let Some(key) = member.worktree_key.as_deref() {
                families.insert(key, index);
            }
        }
    }
    let owner = |workspace: &ClientShellWorkspace| {
        workspace
            .worktree
            .as_ref()
            .and_then(|tree| families.get(tree.key.as_str()))
            .or_else(|| owners.get(workspace.workspace_id.as_str()))
            .copied()
    };
    let mut buckets = vec![Vec::new(); groups.len() + 1];
    let mut counts = vec![0; buckets.len()];
    let mut statuses = vec![crate::api::schema::AgentStatus::Unknown; buckets.len()];
    let focused_family = snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.focused)
        .and_then(|workspace| workspace.worktree.as_ref())
        .map(|tree| tree.key.as_str());
    for workspace in &snapshot.workspaces {
        let index = owner(workspace).unwrap_or(groups.len());
        counts[index] += 1;
        if status_priority(workspace.agent_status) > status_priority(statuses[index]) {
            statuses[index] = workspace.agent_status;
        }
    }
    for entry in entries {
        let workspace = &snapshot.workspaces[entry.index];
        let index = owner(workspace).unwrap_or(groups.len());
        let focused_parent = workspace.worktree.as_ref().is_some_and(|tree| {
            !tree.is_linked_worktree && focused_family == Some(tree.key.as_str())
        });
        if groups.get(index).is_none_or(|group| !group.collapsed)
            || workspace.focused
            || focused_parent
        {
            buckets[index].push(SpaceRow::Workspace {
                entry,
                grouped: true,
            });
        }
    }
    let mut rows = Vec::with_capacity(snapshot.workspaces.len() + groups.len() + 1);
    for (index, bucket) in buckets.into_iter().enumerate() {
        if index == groups.len() && counts[index] == 0 {
            continue;
        }
        rows.push(SpaceRow::Header {
            group: (index < groups.len()).then_some(index),
            count: counts[index],
            status: statuses[index],
        });
        rows.extend(bucket);
    }
    rows
}

pub(super) fn visible_workspaces(
    snapshot: &ClientShellSnapshot,
    collapsed: &HashSet<String>,
    groups: &[SpaceGroup],
) -> Vec<WorkspaceEntry> {
    rows(snapshot, collapsed, groups)
        .iter()
        .filter_map(|row| row.workspace().copied())
        .collect()
}

pub(super) fn render_header(
    buffer: &mut Buffer,
    rect: Rect,
    row: &SpaceRow,
    groups: &[SpaceGroup],
    endpoint: &ClientEndpointId,
    config: &ClientShellConfig,
    hits: &mut ShellHitMap,
) {
    let SpaceRow::Header {
        group,
        count,
        status,
    } = row
    else {
        return;
    };
    let Some(group) = group.and_then(|index| groups.get(index)) else {
        for x in rect.x.saturating_add(3)..rect.right().saturating_sub(1) {
            if let Some(cell) = buffer.cell_mut((x, rect.y)) {
                cell.set_symbol("\u{2500}")
                    .set_style(Style::default().fg(config.palette.surface_dim));
            }
        }
        return;
    };
    let marker = if group.collapsed {
        "\u{25b8}"
    } else {
        "\u{25be}"
    };
    let name = &group.name;
    let suffix = format!(
        " {count} {}",
        status_icon(*status, config.status_indicators)
    );
    let suffix_width = render::display_width(&suffix).min(rect.width);
    render::put_text(
        buffer,
        rect.x,
        rect.y,
        rect.width.saturating_sub(suffix_width),
        &format!(" {marker} {name}"),
        Style::default()
            .fg(config.palette.overlay1)
            .add_modifier(Modifier::BOLD),
    );
    render::put_right_text(
        buffer,
        rect,
        rect.y,
        &suffix,
        Style::default().fg(status_color(*status, &config.palette)),
    );
    hits.space_groups
        .push((rect, endpoint.clone(), group.name.clone()));
}

impl ClientShellState {
    pub(super) fn open_move_to_group(&mut self, workspace_id: String, x: u16, y: u16) {
        let endpoint_id = self.active_endpoint_id.clone();
        let groups = self
            .space_groups
            .get(&endpoint_id)
            .iter()
            .map(|group| group.name.clone())
            .collect();
        self.overlay = Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
            target: ClientContextMenuTarget::MoveToSpaceGroup {
                endpoint_id,
                workspace_id,
                groups,
            },
            x,
            y,
            highlighted: 0,
            scroll: 0,
        }));
    }

    pub(super) fn toggle_space_group(
        &mut self,
        endpoint: &ClientEndpointId,
        name: &str,
        outcome: &mut ClientShellInput,
    ) {
        if let Some(group) = self
            .space_groups
            .get_mut(endpoint)
            .iter_mut()
            .find(|group| group.name == name)
        {
            group.collapsed = !group.collapsed;
            self.persist_chrome_preferences(outcome);
            outcome.repaint = true;
        }
    }

    pub(super) fn assign_space_group(
        &mut self,
        endpoint: &ClientEndpointId,
        workspace_id: &str,
        name: Option<&str>,
        outcome: &mut ClientShellInput,
    ) {
        if let Some(snapshot) = self
            .endpoints
            .iter()
            .find(|candidate| &candidate.endpoint_id == endpoint)
            .and_then(|endpoint| endpoint.snapshot.as_deref())
        {
            self.space_groups
                .assign(endpoint, snapshot, workspace_id, name);
            self.persist_chrome_preferences(outcome);
            self.reveal_focused_workspace = true;
        }
        outcome.repaint = true;
    }

    pub(super) fn delete_space_group(
        &mut self,
        endpoint: &ClientEndpointId,
        name: &str,
        outcome: &mut ClientShellInput,
    ) {
        self.space_groups
            .get_mut(endpoint)
            .retain(|group| group.name != name);
        self.persist_chrome_preferences(outcome);
        self.reveal_focused_workspace = true;
        outcome.repaint = true;
    }

    pub(super) fn save_space_group_name(
        &mut self,
        rename: ClientRenameOverlay,
        outcome: &mut ClientShellInput,
    ) {
        let ClientRenameTarget::SpaceGroup {
            ref endpoint_id,
            ref old_name,
            ref workspace_id,
        } = rename.target
        else {
            return;
        };
        let name = rename.input.trim();
        let groups = self.space_groups.get_mut(endpoint_id);
        if name.is_empty()
            || name.chars().any(char::is_control)
            || groups
                .iter()
                .any(|group| group.name == name && old_name.as_deref() != Some(name))
        {
            self.set_endpoint_error("Group name must be nonempty and unique on this machine");
            self.overlay = Some(ClientShellOverlay::Rename(rename));
            outcome.repaint = true;
            return;
        }
        if let Some(old_name) = old_name {
            if let Some(group) = groups.iter_mut().find(|group| &group.name == old_name) {
                group.name = name.into();
            }
        } else {
            groups.push(SpaceGroup {
                name: name.into(),
                collapsed: false,
                members: Vec::new(),
            });
        }
        if let Some(workspace_id) = workspace_id {
            self.assign_space_group(endpoint_id, workspace_id, Some(name), outcome);
        } else {
            self.persist_chrome_preferences(outcome);
        }
        outcome.repaint = true;
    }
}
