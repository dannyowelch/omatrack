//! Load a fixture, edit it, save, and read the edit back.
//!
//! The editor itself is tested in `src/edit.rs`. This checks the file path:
//! the writer stores the mutation, and undoing every edit restores the bytes.

use std::fs;
use std::path::PathBuf;

use omatrack::edit::{CellRange, Editor, Field, PatternCursor, Place};
use omatrack::Module;

fn fixtures() -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let mut files: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "mod"))
        .collect();
    files.sort();
    files
}

#[test]
fn fixtures_edit_save_reload_and_undo_to_the_original_bytes() {
    let files = fixtures();
    assert!(files.len() >= 8, "expected the fixture set, got {files:?}");
    for path in files {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("module")
            .to_string();
        let original = Module::load(&path).unwrap();
        let original_bytes = original.to_bytes().unwrap();
        let mut module = original.clone();
        let mut editor = Editor::new();

        let place = Place {
            pattern: 0,
            cursor: PatternCursor {
                row: 0,
                channel: 0,
                field: Field::Note,
            },
            step: 1,
        };
        editor.enter_note(&mut module, place, 214, 1).unwrap();
        let mut effect = place;
        effect.cursor.field = Field::Effect;
        effect.cursor.row = 1;
        editor.enter_digit(&mut module, effect, 0x0D).unwrap();
        editor.set_title(&mut module, "Omatrack Edit").unwrap();
        editor.set_sample_name(&mut module, 0, "omatrack").unwrap();
        if module.patterns.len() > 1 {
            let delta = if module.order[0] == 0 { 1 } else { -1 };
            assert!(editor.bump_order_pattern(&mut module, 0, delta));
        }

        assert!(editor.is_dirty(), "{name}");
        let edited = module.clone();
        let out = std::env::temp_dir().join(format!("omatrack-m3-{name}"));
        module.save(&out).unwrap();
        let loaded = Module::load(&out).unwrap();
        assert_eq!(loaded, edited, "{name}");
        assert_eq!(loaded.display_title(), "Omatrack Edit", "{name}");
        assert_eq!(loaded.samples[0].display_name(), "omatrack", "{name}");
        assert_eq!(loaded.patterns[0].rows[0][0].period, 214, "{name}");
        assert_eq!(loaded.patterns[0].rows[0][0].sample, 1, "{name}");
        assert_eq!(loaded.patterns[0].rows[1][0].effect, 0x0D, "{name}");
        let _ = fs::remove_file(&out);

        while editor.undo(&mut module) {}
        assert_eq!(module.to_bytes().unwrap(), original_bytes, "{name}");
        assert!(!editor.is_dirty(), "{name}");
        assert!(!editor.can_undo(), "{name}");
    }
}

#[test]
fn a_block_survives_a_save_of_a_fresh_module() {
    let mut module = Module::default();
    let mut editor = Editor::new();
    let place = Place {
        pattern: 0,
        cursor: PatternCursor {
            row: 2,
            channel: 1,
            field: Field::Note,
        },
        step: 0,
    };
    editor.enter_note(&mut module, place, 856, 3).unwrap();
    let copied = Editor::copy_range(&module, 0, CellRange::single(2, 1));
    editor.paste(&mut module, 0, 10, 3, &copied);
    editor.transpose(&mut module, 0, CellRange::single(10, 3), 2);
    let path = std::env::temp_dir().join(format!("omatrack-m3-block-{}.mod", std::process::id()));
    module.save(&path).unwrap();
    let loaded = Module::load(&path).unwrap();
    assert_eq!(loaded.patterns[0].rows[2][1].period, 856);
    assert_eq!(loaded.patterns[0].rows[2][1].sample, 3);
    assert_eq!(loaded.patterns[0].rows[10][3].sample, 3);
    assert_ne!(loaded.patterns[0].rows[10][3].period, 856);
    let _ = fs::remove_file(path);
    while editor.undo(&mut module) {}
    assert_eq!(
        module.to_bytes().unwrap(),
        Module::default().to_bytes().unwrap()
    );
}
