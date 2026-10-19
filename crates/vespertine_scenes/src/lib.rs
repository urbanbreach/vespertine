//! Scenes and the scene runner for fidelity and performance measurement.
//!
//! Scene sources are written against the public `vespertine` API only; the six `PERF-*` scenes
//! and the runner core are also compiled unchanged against upstream GPUI by the baseline runner
//! (see README.md). Scenes live in `src/perf/<id>.rs` and `src/fid/<id>.rs`; `build.rs` declares
//! every file in those directories as a module and each file registers its scenes with
//! `register_scene!`, so adding a scene never edits a shared list.

mod common;
mod corpus;
mod runner;
mod scene;

mod perf {
    include!(concat!(env!("OUT_DIR"), "/perf_modules.rs"));
}

mod fid {
    include!(concat!(env!("OUT_DIR"), "/fid_modules.rs"));
}

pub use runner::run;
pub use scene::{Scene, SceneEntry, create_scene};

#[cfg(test)]
mod tests {
    use crate::scene::{SceneEntry, create_scene};

    #[test]
    fn baseline_perf_scenes_are_registered() {
        for id in [
            "PERF-LIST",
            "PERF-FLEX",
            "PERF-TEXT",
            "PERF-QUADS",
            "PERF-STREAM",
            "PERF-IDLE",
        ] {
            let scene = create_scene(id).unwrap_or_else(|| panic!("{id} is not registered"));
            assert_eq!(scene.size(), (800.0, 500.0), "{id}");
        }
    }

    #[test]
    fn scene_ids_are_unique_and_unknown_ids_are_absent() {
        let mut ids: Vec<&str> = crate::scene::inventory::iter::<SceneEntry>
            .into_iter()
            .map(|entry| entry.id)
            .collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "duplicate scene ids");
        assert!(create_scene("NO-SUCH-SCENE").is_none());
    }
}
