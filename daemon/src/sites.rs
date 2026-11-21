//! Where a pane is in its space: the grid, popped out (its own split tree, a floating window over
//! the grid), or backgrounded (the space's tray). wheeld holds which, and where each floating
//! window sits and how big it is (`SpaceInfo::frames`), so an agent can arrange them.

use wheel_proto::{Node, PaneFrame, PaneKind, PaneSite};

use crate::server::AppState;
use crate::settings::PLACEMENT;
use crate::spaces::{find_leaf_mut, last_pane, panes, place, take_leaf, Placement};

impl AppState {
    /// Put `pane` in the grid, in a popped tree of its own, or in the tray, keeping its id, its
    /// process and its scrollback. To the grid it goes where `panes.placement` puts a split with
    /// no side, taking a lone empty pane's place if that is all the grid holds. A tree or grid
    /// that loses its last pane is replaced as `pane_close` replaces it.
    pub fn pane_set_site(&self, pane: &str, site: PaneSite) -> Result<(), String> {
        let (space_id, anchor) = {
            let spaces = self.spaces.lock().unwrap();
            let space = &spaces.spaces[spaces.holding(pane)?];
            (space.id.clone(), last_pane(&space.root).to_string())
        };
        let strategy = match site {
            PaneSite::Grid => {
                let value = self.setting(PLACEMENT, Some(&space_id))?;
                Some(Placement::parse(&value).unwrap_or(Placement::Dwindle))
            }
            _ => None,
        };
        let size = match strategy {
            Some(Placement::Dwindle) => self.pane_grid(&anchor),
            _ => None,
        };
        let mut spaces = self.spaces.lock().unwrap();
        let index = spaces.holding(pane)?;
        let (split_id, fresh_empty) = (self.new_id('x'), self.new_id('p'));
        let space = &mut spaces.spaces[index];
        let from = space.site_of(pane).expect("holding found the pane");
        let alone_popped = space
            .popped
            .iter()
            .any(|t| matches!(t, Node::Pane { id, .. } if id == pane));
        if (from == site && site != PaneSite::Popped) || (site == PaneSite::Popped && alone_popped)
        {
            return Ok(());
        }
        let leaf = take_leaf(space, pane, &mut || fresh_empty.clone())
            .ok_or_else(|| format!("no pane {pane}"))?;
        match site {
            PaneSite::Popped => space.popped.push(leaf),
            PaneSite::Backgrounded => space.backgrounded.push(leaf),
            PaneSite::Grid => {
                let lone_empty = matches!(
                    &space.root,
                    Node::Pane {
                        kind: PaneKind::Empty { .. },
                        ..
                    }
                );
                if lone_empty {
                    space.root = leaf;
                } else {
                    // The anchor was the grid's last pane before the take; look again, and use the
                    // measured size only when it is still the one.
                    let target = last_pane(&space.root).to_string();
                    let size = if target == anchor { size } else { None };
                    let strategy = strategy.expect("a grid site has a strategy");
                    place(&mut space.root, &target, strategy, size, &split_id, pane)
                        .ok_or_else(|| format!("no pane {target}"))?;
                    *find_leaf_mut(&mut space.root, pane)
                        .expect("place made a leaf for the pane") = leaf;
                }
            }
        }
        tracing::info!(space = %space.id, %pane, from = from.name(), to = site.name(), placed_by = strategy.map(Placement::name), "pane site changed");
        self.changed(&mut spaces);
        Ok(())
    }

    /// Move `pane`'s floating window to `frame`: every pane of the popped tree holding it takes
    /// the frame. A pane in the grid or the tray is refused, naming it.
    pub fn pane_set_frame(&self, pane: &str, frame: PaneFrame) -> Result<(), String> {
        if !frame.valid() {
            return Err(format!("frame {},{},{},{} for pane {pane} needs finite numbers and a width and height above 0", frame.x, frame.y, frame.w, frame.h));
        }
        let mut spaces = self.spaces.lock().unwrap();
        let index = spaces.holding(pane)?;
        let space = &mut spaces.spaces[index];
        let Some(tree) = space.popped.iter().find(|t| t.holds(pane)) else {
            let site = match space.site_of(pane) {
                Some(PaneSite::Backgrounded) => "backgrounded",
                _ => "in the grid",
            };
            return Err(format!("pane {pane} is {site}, not popped out: pop it out first (wheel pane pop {pane})"));
        };
        let ids: Vec<String> = panes(tree).into_iter().map(|(id, _)| id.to_owned()).collect();
        for id in ids {
            space.frames.insert(id, frame);
        }
        tracing::info!(space = %space.id, %pane, x = frame.x, y = frame.y, w = frame.w, h = frame.h, "pane frame set");
        self.changed(&mut spaces);
        Ok(())
    }
}
