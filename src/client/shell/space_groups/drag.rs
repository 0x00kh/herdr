use super::*;

impl ClientShellState {
    fn space_row_rect(
        &self,
        endpoint: &ClientEndpointId,
        snapshot: &ClientShellSnapshot,
        row: &SpaceRow,
    ) -> Option<Rect> {
        match row {
            SpaceRow::Header {
                group: Some(index), ..
            } => {
                let name = &self.space_groups.get(endpoint).get(*index)?.name;
                self.hits
                    .space_groups
                    .iter()
                    .find(|(_, id, candidate)| id == endpoint && candidate == name)
                    .map(|(rect, _, _)| *rect)
            }
            SpaceRow::Workspace { entry, .. } => {
                let id = &snapshot.workspaces[entry.index].workspace_id;
                self.hits
                    .workspaces
                    .iter()
                    .find(|hit| &hit.endpoint_id == endpoint && &hit.workspace_id == id)
                    .map(|hit| hit.rect)
            }
            SpaceRow::Header { group: None, .. } => None,
        }
    }

    fn space_drop_contains(&self, endpoint: &ClientEndpointId, point: (u16, u16)) -> bool {
        let body = self.hits.workspace_body;
        !self.sidebar_collapsed
            && point.0 >= body.x
            && point.0 < body.right()
            && !super::super::contains(self.hits.workspace_scrollbar, point)
            && !self
                .hits
                .machines
                .iter()
                .any(|hit| super::super::contains(hit.rect, point))
            && !self
                .hits
                .workspaces
                .iter()
                .any(|hit| &hit.endpoint_id != endpoint && super::super::contains(hit.rect, point))
            && !self
                .hits
                .space_groups
                .iter()
                .any(|(rect, id, _)| id != endpoint && super::super::contains(*rect, point))
    }

    pub(in crate::client::shell) fn space_group_drop_target_at(
        &self,
        endpoint: &ClientEndpointId,
        source: &str,
        point: (u16, u16),
    ) -> Option<(Option<String>, u16)> {
        if !self.space_drop_contains(endpoint, point) {
            return None;
        }
        let snapshot = self
            .endpoints
            .iter()
            .find(|candidate| &candidate.endpoint_id == endpoint)?
            .snapshot
            .as_deref()?;
        let empty = HashSet::new();
        let groups = self.space_groups.get(endpoint);
        let source_index = groups.iter().position(|group| group.name == source)?;
        let rows = rows(
            snapshot,
            self.collapsed_groups_for_endpoint(endpoint)
                .unwrap_or(&empty),
            groups,
        );
        let mut slots = Vec::new();
        let mut current_group = None;
        let mut hovered_group = None;
        for (position, row) in rows.iter().enumerate() {
            if let SpaceRow::Header { group, .. } = row {
                current_group = *group;
            }
            let Some(index) = current_group else { continue };
            let Some(rect) = self.space_row_rect(endpoint, snapshot, row) else {
                continue;
            };
            if super::super::contains(rect, point) {
                hovered_group = Some(index);
            }
            if matches!(row, SpaceRow::Header { .. }) {
                slots.push((Some(groups[index].name.clone()), rect.y));
            }
            // Only offer the end of a group when its last displayed row is visible.
            if rows
                .get(position + 1)
                .is_none_or(|next| matches!(next, SpaceRow::Header { .. }))
                && rect.bottom() < self.hits.workspace_body.bottom()
            {
                slots.push((
                    groups.get(index + 1).map(|group| group.name.clone()),
                    rect.bottom(),
                ));
            }
        }
        if let Some(index) = hovered_group {
            if index > source_index {
                let before = groups.get(index + 1).map(|group| group.name.as_str());
                return slots.into_iter().find(|(id, _)| id.as_deref() == before);
            }
            return slots
                .into_iter()
                .rev()
                .find(|(id, _)| id.as_deref() == Some(groups[index].name.as_str()));
        }
        nearest_slot(slots, point.1)
    }

    pub(in crate::client::shell) fn move_space_group(
        &mut self,
        endpoint: &ClientEndpointId,
        name: &str,
        before: Option<&str>,
        outcome: &mut ClientShellInput,
    ) {
        let groups = self.space_groups.get_mut(endpoint);
        let Some(source) = groups.iter().position(|group| group.name == name) else {
            return;
        };
        let target = match before {
            Some(name) => match groups.iter().position(|group| group.name == name) {
                Some(index) => index,
                None => return,
            },
            None => groups.len(),
        };
        if target == source || target == source + 1 {
            return;
        }
        let group = groups.remove(source);
        groups.insert(target - usize::from(target > source), group);
        self.persist_chrome_preferences(outcome);
        outcome.repaint = true;
    }

    pub(in crate::client::shell) fn grouped_workspace_drop_target_at(
        &self,
        source: &str,
        point: (u16, u16),
    ) -> Option<(Option<String>, u16)> {
        let endpoint = &self.active_endpoint_id;
        if !self.space_drop_contains(endpoint, point) {
            return None;
        }
        let snapshot = self.snapshot.as_deref()?;
        let groups = self.space_groups.get(endpoint);
        let owner = self.space_groups.workspace_group(endpoint, source);
        if owner.is_some_and(|index| groups[index].collapsed) {
            return None;
        }
        let empty = HashSet::new();
        let rows = rows(
            snapshot,
            self.collapsed_groups_for_endpoint(endpoint)
                .unwrap_or(&empty),
            groups,
        );
        let mut current_group = None;
        let mut entries = Vec::new();
        for row in &rows {
            match row {
                SpaceRow::Header { group, .. } => current_group = *group,
                SpaceRow::Workspace { entry, .. } if current_group == owner => {
                    entries.push((row, entry))
                }
                _ => {}
            }
        }
        let source_position = entries
            .iter()
            .position(|(_, entry)| snapshot.workspaces[entry.index].workspace_id == source)?;
        let mut slots = Vec::new();
        let mut root_position = None;
        let mut hovered_root = None;
        for (position, (row, entry)) in entries.iter().enumerate() {
            let workspace = &snapshot.workspaces[entry.index];
            let is_root = !entry.indented
                && !workspace
                    .worktree
                    .as_ref()
                    .is_some_and(|tree| tree.is_linked_worktree);
            if is_root {
                root_position = Some(position);
            }
            let Some(rect) = self.space_row_rect(endpoint, snapshot, row) else {
                continue;
            };
            if super::super::contains(rect, point) {
                hovered_root = root_position;
            }
            if is_root {
                slots.push((Some(workspace.workspace_id.clone()), rect.y));
            }
            let next = entries.get(position + 1).map(|(_, entry)| *entry);
            if next.is_none_or(|entry| !entry.indented)
                && rect.bottom() < self.hits.workspace_body.bottom()
            {
                slots.push((
                    next.map(|entry| snapshot.workspaces[entry.index].workspace_id.clone()),
                    rect.bottom(),
                ));
            }
        }
        // A row is a destination, not always a "before" boundary. In particular,
        // one terminal cell cannot distinguish the two halves of a single-line row.
        if let Some(position) = hovered_root {
            if position > source_position {
                let next = entries
                    .iter()
                    .skip(position + 1)
                    .find(|(_, entry)| !entry.indented);
                let before =
                    next.map(|(_, entry)| snapshot.workspaces[entry.index].workspace_id.as_str());
                return slots.into_iter().find(|(id, _)| id.as_deref() == before);
            }
            let before = &snapshot.workspaces[entries[position].1.index].workspace_id;
            return slots
                .into_iter()
                .rev()
                .find(|(id, _)| id.as_ref() == Some(before));
        }
        nearest_slot(slots, point.1)
    }
}

fn nearest_slot(slots: Vec<(Option<String>, u16)>, row: u16) -> Option<(Option<String>, u16)> {
    if row < slots.first()?.1 || row > slots.last()?.1 {
        return None;
    }
    slots
        .into_iter()
        .enumerate()
        .min_by_key(|(index, (_, y))| (row.abs_diff(*y), *index))
        .map(|(_, slot)| slot)
}
