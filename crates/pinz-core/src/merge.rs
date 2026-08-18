//! Three-way merge of a single pin file, for sync conflicts.
//!
//! A pin file has two layers that fail differently: the **content** (title and
//! body, the part a human wrote) and the **cosmetics** (the frontmatter
//! position fields plus color, the part the board writes when pins are moved
//! around). Git sees one text file and conflicts on either; this module knows
//! the difference, so a pin moved on one machine and edited on the other can
//! merge cleanly instead of stopping the sync.
//!
//! The rules are in `design/specs/2026-08-18-sync-conflict-handling.md`:
//! content merges only when at most one side changed it; each cosmetic field
//! takes the side that changed it, and a cosmetic tie goes to the local side.
//! Anything this module cannot decide returns `None`, and the caller stops
//! exactly as before - this narrows what counts as a guess, it does not remove
//! the stop.

use crate::file_store::{parse_pin, render_pin};
use crate::model::Note;

/// Try to merge one conflicted pin file.
///
/// `local` is this machine's version, `remote` the other machine's, `base`
/// their common ancestor (absent when both machines created the file
/// independently). Returns the merged file contents, or `None` when both
/// sides changed the content differently - that is a judgement call and a
/// human makes it.
pub fn merge_pin(base: Option<&str>, remote: &str, local: &str) -> Option<String> {
    // The id is transport, not data: it exists so a loaded board can address
    // notes, and never reaches the file. Any value works here.
    let base = base.map(|b| parse_pin(b, 0));
    let remote = parse_pin(remote, 0);
    let local = parse_pin(local, 0);

    let (title, body) = merge_content(base.as_ref(), &remote, &local)?;
    let merged = Note {
        id: 0,
        title,
        body,
        x: pick(base.as_ref().map(|b| b.x), remote.x, local.x),
        y: pick(base.as_ref().map(|b| b.y), remote.y, local.y),
        z: pick(base.as_ref().map(|b| b.z), remote.z, local.z),
        color: pick(base.as_ref().map(|b| b.color), remote.color, local.color),
    };
    Some(render_pin(&merged))
}

/// The content layer: merges only when at most one side changed it.
fn merge_content(base: Option<&Note>, remote: &Note, local: &Note) -> Option<(String, String)> {
    let content = |n: &Note| (n.title.clone(), n.body.clone());
    let local_content = content(local);
    let remote_content = content(remote);
    if local_content == remote_content {
        return Some(local_content);
    }
    let base_content = content(base?);
    if local_content == base_content {
        return Some(remote_content);
    }
    if remote_content == base_content {
        return Some(local_content);
    }
    None
}

/// One cosmetic field: the side that changed it wins, and a tie (both changed,
/// or no base to compare against) goes to local - the person at this board
/// arranged it this way most recently in their own view.
fn pick<T: PartialEq>(base: Option<T>, remote: T, local: T) -> T {
    match base {
        Some(base) if local == base => remote,
        _ => local,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin(x: i64, y: i64, z: i64, color: &str, title: &str, body: &str) -> String {
        let body_part = if body.is_empty() {
            String::new()
        } else {
            format!("\n{body}\n")
        };
        format!("---\nx: {x}\ny: {y}\nz: {z}\ncolor: {color}\n---\n# {title}\n{body_part}")
    }

    /// The 2026-08-07 incident: one machine rewrote the body, the other only
    /// nudged the pin's stacking order. Both survive.
    #[test]
    fn a_body_edit_here_and_a_position_bump_there_merge_into_one_pin() {
        let base = pin(10, 20, 5, "green", "Keeper", "old todo");
        let remote = pin(10, 20, 6, "green", "Keeper", "old todo");
        let local = pin(10, 20, 5, "green", "Keeper", "go with Keeper Commander");

        let merged = merge_pin(Some(&base), &remote, &local).expect("should merge");
        assert!(
            merged.contains("go with Keeper Commander"),
            "local body kept:\n{merged}"
        );
        assert!(merged.contains("z: 6"), "remote position kept:\n{merged}");
    }

    #[test]
    fn a_body_edit_there_and_a_move_here_merge_the_same_way() {
        let base = pin(10, 20, 5, "green", "Keeper", "old todo");
        let remote = pin(10, 20, 5, "green", "Keeper", "rewritten remotely");
        let local = pin(90, 20, 5, "green", "Keeper", "old todo");

        let merged = merge_pin(Some(&base), &remote, &local).expect("should merge");
        assert!(
            merged.contains("rewritten remotely"),
            "remote body kept:\n{merged}"
        );
        assert!(merged.contains("x: 90"), "local position kept:\n{merged}");
    }

    #[test]
    fn both_bodies_changed_differently_is_not_ours_to_resolve() {
        let base = pin(0, 0, 1, "yellow", "t", "original");
        let remote = pin(0, 0, 1, "yellow", "t", "remote version");
        let local = pin(0, 0, 1, "yellow", "t", "local version");
        assert_eq!(merge_pin(Some(&base), &remote, &local), None);
    }

    #[test]
    fn a_title_change_counts_as_content() {
        let base = pin(0, 0, 1, "yellow", "old title", "body");
        let remote = pin(0, 0, 1, "yellow", "remote title", "body");
        let local = pin(0, 0, 1, "yellow", "local title", "body");
        assert_eq!(merge_pin(Some(&base), &remote, &local), None);
    }

    #[test]
    fn the_same_edit_made_on_both_machines_merges() {
        let base = pin(0, 0, 1, "yellow", "t", "original");
        let remote = pin(5, 0, 1, "yellow", "t", "same new text");
        let local = pin(0, 0, 1, "yellow", "t", "same new text");
        let merged = merge_pin(Some(&base), &remote, &local).expect("should merge");
        assert!(merged.contains("same new text"));
        assert!(merged.contains("x: 5"), "remote's move survives:\n{merged}");
    }

    #[test]
    fn a_cosmetic_tie_goes_to_the_local_side() {
        let base = pin(0, 0, 1, "yellow", "t", "body");
        let remote = pin(40, 0, 8, "yellow", "t", "body");
        let local = pin(70, 0, 3, "yellow", "t", "body");
        let merged = merge_pin(Some(&base), &remote, &local).expect("should merge");
        assert!(
            merged.contains("x: 70") && merged.contains("z: 3"),
            "local wins ties:\n{merged}"
        );
    }

    #[test]
    fn each_cosmetic_field_merges_on_its_own() {
        // Remote moved the pin, local recolored it: both changes survive.
        let base = pin(0, 0, 1, "yellow", "t", "body");
        let remote = pin(40, 25, 1, "yellow", "t", "body");
        let local = pin(0, 0, 1, "red", "t", "body");
        let merged = merge_pin(Some(&base), &remote, &local).expect("should merge");
        assert!(
            merged.contains("x: 40") && merged.contains("y: 25"),
            "{merged}"
        );
        assert!(merged.contains("color: red"), "{merged}");
    }

    #[test]
    fn without_a_base_matching_content_still_merges() {
        let remote = pin(40, 0, 2, "yellow", "t", "same body");
        let local = pin(70, 0, 2, "yellow", "t", "same body");
        let merged = merge_pin(None, &remote, &local).expect("should merge");
        assert!(
            merged.contains("x: 70"),
            "local wins without a base:\n{merged}"
        );
    }

    #[test]
    fn without_a_base_differing_content_stops() {
        let remote = pin(0, 0, 1, "yellow", "t", "one body");
        let local = pin(0, 0, 1, "yellow", "t", "another body");
        assert_eq!(merge_pin(None, &remote, &local), None);
    }

    #[test]
    fn the_merged_file_is_a_well_formed_pin() {
        let base = pin(10, 20, 5, "green", "Keeper", "old");
        let remote = pin(10, 20, 6, "green", "Keeper", "old");
        let local = pin(10, 20, 5, "green", "Keeper", "new text");
        let merged = merge_pin(Some(&base), &remote, &local).unwrap();
        let note = parse_pin(&merged, 1);
        assert_eq!(note.title, "Keeper");
        assert_eq!(note.body, "new text");
        assert_eq!(note.z, 6);
        assert_eq!(merged, render_pin(&note), "output is renderer-normal");
    }
}
