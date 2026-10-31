//! 2D grid projection and spatial neighbor finding for pane layouts.
//!
//! Projects the recursive `LayoutNode` tree into flat `PaneRect` rectangles
//! with normalised viewport coordinates (0.0-1.0), then uses a 90-degree cone
//! filter to find the nearest neighbor in a given direction.

use super::ops::Direction;
use super::state::{Axis, LayoutNode};
use crate::ids::*;

/// A pane's computed 2D position in the viewport. All values are relative (0.0–1.0).
#[derive(Clone, Debug)]
pub struct PaneRect {
    pub pane_id: PaneId,
    pub slot_id: NodeId,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Project a layout tree into a flat list of 2D pane rectangles.
pub fn project_grid(root: &LayoutNode) -> Vec<PaneRect> {
    let mut rects = Vec::new();
    project_node(root, 0.0, 0.0, 1.0, 1.0, &mut rects);
    rects
}

fn project_node(node: &LayoutNode, x: f64, y: f64, w: f64, h: f64, out: &mut Vec<PaneRect>) {
    match node {
        LayoutNode::Slot { id, pane_ids, .. } => {
            for pane_id in pane_ids {
                out.push(PaneRect {
                    pane_id: pane_id.clone(),
                    slot_id: id.clone(),
                    x,
                    y,
                    w,
                    h,
                });
            }
        }
        LayoutNode::Split {
            axis,
            children,
            sizes,
            ..
        } => {
            let sizes = if sizes.len() == children.len() {
                sizes.clone()
            } else {
                vec![1.0 / children.len() as f64; children.len()]
            };

            let mut offset = 0.0;
            for (i, child) in children.iter().enumerate() {
                let s = sizes[i];
                match axis {
                    Axis::Horizontal => {
                        project_node(child, x + offset * w, y, s * w, h, out);
                    }
                    Axis::Vertical => {
                        project_node(child, x, y + offset * h, w, s * h, out);
                    }
                }
                offset += s;
            }
        }
    }
}

/// Find the nearest pane in the given direction from the source pane.
/// Uses a 90-degree cone filter. Returns None if no neighbor found.
pub fn find_neighbor(root: &LayoutNode, pane_id: &PaneId, dir: &Direction) -> Option<PaneId> {
    let rects = project_grid(root);

    let src = rects.iter().find(|r| &r.pane_id == pane_id)?;
    let src_cx = src.x + src.w / 2.0;
    let src_cy = src.y + src.h / 2.0;
    let src_slot = &src.slot_id;

    let mut best_dist = f64::INFINITY;
    let mut best_pane = None;

    for r in &rects {
        // Skip panes in the same slot (stacked/tabbed — not spatial neighbors)
        if &r.slot_id == src_slot {
            continue;
        }

        let cx = r.x + r.w / 2.0;
        let cy = r.y + r.h / 2.0;
        let dx = cx - src_cx;
        let dy = cy - src_cy;

        // 90-degree cone filter: primary axis must dominate
        let in_cone = match dir {
            Direction::Right => dx > 0.0 && dy.abs() <= dx,
            Direction::Left => dx < 0.0 && dy.abs() <= -dx,
            Direction::Below => dy > 0.0 && dx.abs() <= dy,
            Direction::Above => dy < 0.0 && dx.abs() <= -dy,
        };

        if !in_cone {
            continue;
        }

        let dist = dx * dx + dy * dy;
        if dist < best_dist {
            best_dist = dist;
            best_pane = Some(r.pane_id.clone());
        }
    }

    best_pane
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_pane_horizontal() -> LayoutNode {
        LayoutNode::new_split(
            NodeId::from("split1"),
            Axis::Horizontal,
            vec![
                LayoutNode::new_slot(NodeId::from("s1"), vec![PaneId::from("left")]),
                LayoutNode::new_slot(NodeId::from("s2"), vec![PaneId::from("right")]),
            ],
            vec![0.5, 0.5],
        )
    }

    #[test]
    fn project_grid_two_panes() {
        let root = two_pane_horizontal();
        let rects = project_grid(&root);
        assert_eq!(rects.len(), 2);

        let left = rects
            .iter()
            .find(|r| r.pane_id == PaneId::from("left"))
            .unwrap();
        assert!((left.x - 0.0).abs() < 0.001);
        assert!((left.w - 0.5).abs() < 0.001);

        let right = rects
            .iter()
            .find(|r| r.pane_id == PaneId::from("right"))
            .unwrap();
        assert!((right.x - 0.5).abs() < 0.001);
        assert!((right.w - 0.5).abs() < 0.001);
    }

    #[test]
    fn project_grid_nested() {
        // [left | [top / bottom]]
        let root = LayoutNode::new_split(
            NodeId::from("hsplit"),
            Axis::Horizontal,
            vec![
                LayoutNode::new_slot(NodeId::from("s1"), vec![PaneId::from("left")]),
                LayoutNode::new_split(
                    NodeId::from("vsplit"),
                    Axis::Vertical,
                    vec![
                        LayoutNode::new_slot(NodeId::from("s2"), vec![PaneId::from("top")]),
                        LayoutNode::new_slot(NodeId::from("s3"), vec![PaneId::from("bottom")]),
                    ],
                    vec![0.5, 0.5],
                ),
            ],
            vec![0.5, 0.5],
        );

        let rects = project_grid(&root);
        assert_eq!(rects.len(), 3);

        let top = rects
            .iter()
            .find(|r| r.pane_id == PaneId::from("top"))
            .unwrap();
        assert!((top.x - 0.5).abs() < 0.001);
        assert!((top.y - 0.0).abs() < 0.001);
        assert!((top.w - 0.5).abs() < 0.001);
        assert!((top.h - 0.5).abs() < 0.001);
    }

    #[test]
    fn find_neighbor_right() {
        let root = two_pane_horizontal();
        let result = find_neighbor(&root, &PaneId::from("left"), &Direction::Right);
        assert_eq!(result, Some(PaneId::from("right")));
    }

    #[test]
    fn find_neighbor_left() {
        let root = two_pane_horizontal();
        let result = find_neighbor(&root, &PaneId::from("right"), &Direction::Left);
        assert_eq!(result, Some(PaneId::from("left")));
    }

    #[test]
    fn find_neighbor_none_in_wrong_direction() {
        let root = two_pane_horizontal();
        let result = find_neighbor(&root, &PaneId::from("left"), &Direction::Left);
        assert_eq!(result, None);
    }

    #[test]
    fn find_neighbor_vertical() {
        let root = LayoutNode::new_split(
            NodeId::from("split1"),
            Axis::Vertical,
            vec![
                LayoutNode::new_slot(NodeId::from("s1"), vec![PaneId::from("top")]),
                LayoutNode::new_slot(NodeId::from("s2"), vec![PaneId::from("bottom")]),
            ],
            vec![0.5, 0.5],
        );
        assert_eq!(
            find_neighbor(&root, &PaneId::from("top"), &Direction::Below),
            Some(PaneId::from("bottom"))
        );
        assert_eq!(
            find_neighbor(&root, &PaneId::from("bottom"), &Direction::Above),
            Some(PaneId::from("top"))
        );
    }

    #[test]
    fn stacked_panes_not_neighbors() {
        let root = LayoutNode::new_split(
            NodeId::from("split1"),
            Axis::Horizontal,
            vec![
                LayoutNode::Slot {
                    id: NodeId::from("s1"),
                    pane_ids: vec![PaneId::from("tab1"), PaneId::from("tab2")],
                    active_pane_id: Some(PaneId::from("tab1")),
                },
                LayoutNode::new_slot(NodeId::from("s2"), vec![PaneId::from("right")]),
            ],
            vec![0.5, 0.5],
        );

        // tab1 and tab2 share a slot — not spatial neighbors
        assert_eq!(
            find_neighbor(&root, &PaneId::from("tab1"), &Direction::Right),
            Some(PaneId::from("right"))
        );
        assert_eq!(
            find_neighbor(&root, &PaneId::from("tab1"), &Direction::Left),
            None
        );
    }
}
