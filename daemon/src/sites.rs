//! Where a pane is in its space: the grid, popped out (its own split tree, a floating window over
//! the grid), or backgrounded (the space's tray). wheeld holds which; each client holds where its
//! windows sit and how big they are (ADR-0022).

use wheel_proto::{Node, PaneKind, PaneSite};

use crate::server::AppState;
use crate::settings::PLACEMENT;
use crate::spaces::{find_leaf_mut, last_pane, place, take_leaf, Placement};

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
        self.changed(&spaces);
        Ok(())
    }
}
