use super::*;
use gpui::{EntityInputHandler, TestAppContext, VisualTestContext};

fn fixture() -> BinaryImage {
    BinaryImage::parse(vec![0xb8, 1, 0, 0, 0, 0xb9, 2, 0, 0, 0, 0xc3]).unwrap()
}

fn update<R>(
    workbench: &Entity<Workbench>,
    cx: &mut VisualTestContext,
    operation: impl FnOnce(&mut Workbench, &mut Context<Workbench>) -> R,
) -> R {
    cx.update(|_, cx| workbench.update(cx, operation))
}

fn command(
    workbench: &Entity<Workbench>,
    command: Command,
    text: &str,
    cx: &mut VisualTestContext,
) {
    update(workbench, cx, |workbench, _| {
        workbench.command = command;
    });
    replace_input(workbench, text, cx);
    // Let input observation finish before delivering the separate Run action.
    update(workbench, cx, |workbench, cx| workbench.run(cx));
}

fn replace_input(workbench: &Entity<Workbench>, text: &str, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        let input = workbench.read(cx).input.clone();
        input.update(cx, |input, cx| {
            let range = 0..input.text().encode_utf16().count();
            input.replace_text_in_range(Some(range), text, window, cx);
        });
    });
}

fn patch(workbench: &Entity<Workbench>, text: &str, cx: &mut VisualTestContext) {
    command(workbench, Command::Patch, text, cx);
    update(workbench, cx, |workbench, cx| {
        assert!(workbench.preview.is_some(), "{}", workbench.message);
        workbench.apply_patch(cx);
    });
    cx.run_until_parked();
    update(workbench, cx, |workbench, _| assert!(!workbench.is_busy()));
}

fn new_workbench(_: &mut Window, cx: &mut Context<Workbench>) -> Workbench {
    let mut workbench = Workbench::new(PathBuf::from("fixture.bin"), fixture(), cx);
    workbench.architecture = Architecture::X86_64;
    workbench.decode();
    workbench
}

#[gpui::test]
fn shared_location_survives_lens_changes_and_invalid_navigation(cx: &mut TestAppContext) {
    let (workbench, cx) = cx.add_window_view(new_workbench);
    update(&workbench, cx, |workbench, cx| workbench.go(5, cx));
    for lens in [
        Lens::Bytes,
        Lens::Assembly,
        Lens::Flow,
        Lens::Strings,
        Lens::Overview,
    ] {
        cx.update(|window, cx| {
            workbench.update(cx, |workbench, cx| {
                workbench.show(lens, window, cx);
                assert!(workbench.lens == lens);
                assert_eq!(workbench.offset, 5);
                assert_eq!(workbench.instructions[0].bytes, [0xb9, 2, 0, 0, 0]);
                assert!(!workbench.is_dirty());
            });
        });
    }
    command(&workbench, Command::Go, "@ffff", cx);
    update(&workbench, cx, |workbench, _| {
        assert_eq!(workbench.offset, 5);
        assert!(workbench.message.contains("outside"));
    });
    command(&workbench, Command::Patch, "90", cx);
    update(&workbench, cx, |workbench, cx| {
        assert!(workbench.preview.is_some());
        workbench.go(0, cx);
        assert!(workbench.preview.is_none());
        assert_eq!(workbench.back, [0, 5]);
    });
}

#[gpui::test]
fn patch_preview_history_and_export_snapshot_keep_newer_edits_dirty(cx: &mut TestAppContext) {
    let (workbench, cx) = cx.add_window_view(new_workbench);
    let original = fixture().bytes().to_vec();
    command(&workbench, Command::Patch, "b8 2a 00 00 00", cx);
    update(&workbench, cx, |workbench, _| {
        assert_eq!(workbench.image.bytes(), original);
        assert!(!workbench.is_dirty());
        assert_eq!(workbench.preview, Some((0, vec![0xb8, 42, 0, 0, 0], 0)));
    });
    update(&workbench, cx, |workbench, cx| workbench.apply_patch(cx));
    cx.run_until_parked();
    let (exported_image, exported_revision) = update(&workbench, cx, |workbench, _| {
        assert!(workbench.is_dirty());
        assert_eq!(&workbench.image.bytes()[..5], [0xb8, 42, 0, 0, 0]);
        workbench.snapshot()
    });
    patch(&workbench, "b8 2b 00 00 00", cx);
    update(&workbench, cx, |workbench, cx| {
        workbench.mark_exported(exported_revision, cx);
        assert!(workbench.is_dirty());
        assert!(workbench.message.contains("newer patches"));
        assert_eq!(&exported_image.bytes()[..5], [0xb8, 42, 0, 0, 0]);
        assert_eq!(&workbench.image.bytes()[..5], [0xb8, 43, 0, 0, 0]);
        workbench.history(false, cx);
        assert!(!workbench.is_dirty());
        assert_eq!(workbench.revision, exported_revision);
        assert_eq!(&workbench.image.bytes()[..5], [0xb8, 42, 0, 0, 0]);
        workbench.history(true, cx);
        assert!(workbench.is_dirty());
        assert_eq!(&workbench.image.bytes()[..5], [0xb8, 43, 0, 0, 0]);
        workbench.history(false, cx);
    });
    patch(&workbench, "b8 2c 00 00 00", cx);
    update(&workbench, cx, |workbench, cx| {
        assert!(workbench.redo.is_empty());
        let revision = workbench.revision;
        workbench.history(true, cx);
        assert_eq!(workbench.revision, revision);
        workbench.mark_exported(revision, cx);
        assert!(!workbench.is_dirty());
    });
}

#[gpui::test]
fn invalid_preview_and_noop_patch_never_change_bytes(cx: &mut TestAppContext) {
    let (workbench, cx) = cx.add_window_view(new_workbench);
    patch(&workbench, "b8 01 00 00 00", cx);
    update(&workbench, cx, |workbench, _| {
        assert!(!workbench.is_dirty());
        assert!(workbench.undo.is_empty());
    });
    command(&workbench, Command::Patch, "90", cx);
    replace_input(&workbench, "not hex", cx);
    update(&workbench, cx, |workbench, _| {
        assert!(
            workbench.preview.is_none(),
            "Editing the command must invalidate the preview before Run is pressed."
        );
    });
    command(&workbench, Command::Patch, "90", cx);
    command(&workbench, Command::Patch, "not hex", cx);
    update(&workbench, cx, |workbench, cx| {
        assert!(
            workbench.preview.is_none(),
            "An invalid command must clear any earlier patch preview."
        );
        workbench.apply_patch(cx);
        assert_eq!(workbench.image.bytes(), fixture().bytes());
        assert!(!workbench.is_dirty());
    });
    command(&workbench, Command::Patch, "90", cx);
    update(&workbench, cx, |workbench, cx| {
        workbench.preview.as_mut().unwrap().2 = workbench.revision + 1;
        workbench.apply_patch(cx);
        assert!(workbench.preview.is_none());
        assert!(workbench.message.contains("Stale"));
        assert!(!workbench.is_dirty());
    });
}

#[gpui::test]
fn assembler_previews_only_equal_length_and_current_location(cx: &mut TestAppContext) {
    if !std::path::Path::new("/usr/bin/nasm").is_file() {
        eprintln!(
            "SKIP: NASM unavailable at /usr/bin/nasm; native workbench assembly acceptance remains unverified."
        );
        return;
    }
    let (workbench, cx) = cx.add_window_view(new_workbench);
    command(&workbench, Command::Assemble, "mov eax, 42", cx);
    cx.run_until_parked();
    update(&workbench, cx, |workbench, _| {
        assert_eq!(workbench.preview, Some((0, vec![0xb8, 42, 0, 0, 0], 0)));
        assert_eq!(workbench.image.bytes(), fixture().bytes());
    });
    command(&workbench, Command::Assemble, "nop", cx);
    cx.run_until_parked();
    update(&workbench, cx, |workbench, _| {
        assert!(
            workbench.preview.is_none(),
            "A rejected assembly must not leave an older preview applicable."
        );
        assert!(workbench.message.contains("sizes must match"));
        assert!(!workbench.is_dirty());
    });
    command(&workbench, Command::Assemble, "mov eax, 42", cx);
    update(&workbench, cx, |workbench, cx| workbench.go(5, cx));
    cx.run_until_parked();
    update(&workbench, cx, |workbench, _| {
        assert!(workbench.preview.is_none());
        assert_eq!(workbench.offset, 5);
        assert!(!workbench.is_dirty());
    });
    command(&workbench, Command::Assemble, "mov ecx, 42", cx);
    replace_input(&workbench, "mov ecx, 43", cx);
    cx.run_until_parked();
    update(&workbench, cx, |workbench, _| {
        assert!(
            workbench.preview.is_none(),
            "Editing input while assembly runs must invalidate its result."
        );
        assert!(!workbench.is_dirty());
    });
    command(&workbench, Command::Assemble, "mov ecx, 99", cx);
    cx.run_until_parked();
    update(&workbench, cx, |workbench, cx| workbench.apply_patch(cx));
    cx.run_until_parked();
    update(&workbench, cx, |workbench, _| {
        assert_eq!(&workbench.image.bytes()[5..10], [0xb9, 99, 0, 0, 0]);
        assert!(workbench.is_dirty());
    });
}

#[gpui::test]
fn command_input_enter_runs_without_inserting_a_newline(cx: &mut TestAppContext) {
    let (workbench, cx) = cx.add_window_view(new_workbench);
    cx.update(|window, cx| {
        workbench.update(cx, |workbench, cx| {
            workbench.command = Command::Go;
            workbench.input.read(cx).focus_handle(cx).focus(window);
        });
    });
    cx.simulate_input("@5");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    update(&workbench, cx, |workbench, cx| {
        assert_eq!(workbench.offset, 5);
        assert_eq!(workbench.input.read(cx).text(), "@5");
    });
}

#[gpui::test]
fn composition_and_focus_notifications_keep_preview_but_content_edits_clear_it(
    cx: &mut TestAppContext,
) {
    let (workbench, cx) = cx.add_window_view(new_workbench);
    command(&workbench, Command::Patch, "90", cx);
    let (preview, generation) = update(&workbench, cx, |workbench, _| {
        assert!(workbench.preview.is_some());
        (workbench.preview.clone(), workbench.input_generation)
    });
    cx.update(|window, cx| {
        let input = workbench.read(cx).input.clone();
        input.read(cx).focus_handle(cx).focus(window);
        input.update(cx, |input, cx| {
            // Native focus/composition transitions can unmark unchanged text.
            input.unmark_text(window, cx);
        });
    });
    cx.simulate_keystrokes("left right");
    cx.update(|window, cx| workbench.read(cx).focus_handle(cx).focus(window));
    cx.write_to_clipboard(ClipboardItem::new_string("90".into()));
    cx.run_until_parked();
    update(&workbench, cx, |workbench, cx| {
        assert_eq!(workbench.input.read(cx).text(), "90");
        assert_eq!(workbench.input_generation, generation);
        assert_eq!(
            workbench.preview, preview,
            "Focus, cursor and unmark notifications must keep Apply available when command content is unchanged."
        );
        workbench.apply_patch(cx);
    });
    cx.run_until_parked();
    update(&workbench, cx, |workbench, _| {
        assert_eq!(workbench.image.bytes()[0], 0x90);
        assert!(workbench.is_dirty());
    });
    command(&workbench, Command::Patch, "91", cx);
    replace_input(&workbench, "92", cx);
    update(&workbench, cx, |workbench, _| {
        assert!(
            workbench.preview.is_none(),
            "An actual native text edit must still invalidate the old preview."
        );
        assert!(workbench.input_generation > generation);
        assert_eq!(workbench.image.bytes()[0], 0x90);
    });
}

fn text_state(shell: &Entity<crate::AppShell>, cx: &mut VisualTestContext) -> (String, bool) {
    cx.update(|_, cx| {
        let editor = shell.read(cx).editor.read(cx);
        (editor.text(), editor.is_dirty())
    })
}

fn load_binary(
    shell: &Entity<crate::AppShell>,
    path: PathBuf,
    cx: &mut VisualTestContext,
) -> Entity<Workbench> {
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.request_binary_replace(
                crate::BinaryIntent::Open(path, Box::new(fixture())),
                window,
                cx,
            );
        });
    });
    cx.run_until_parked();
    cx.update(|_, cx| shell.read(cx).workbench.clone().unwrap())
}

#[gpui::test]
fn binary_document_tab_resumes_lens_and_scroll(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let (shell, cx) = cx.add_window_view(crate::AppShell::new);
    let workbench = load_binary(&shell, directory.path().join("fixture.bin"), cx);
    for lens in [Lens::Bytes, Lens::Strings, Lens::Overview] {
        for switch_from_text in [false, true] {
            cx.update(|window, cx| {
                shell.update(cx, |shell, cx| {
                    if switch_from_text {
                        shell.show_editor(window, cx);
                    }
                    let scroll = gpui::point(px(-24.0), px(-46.0));
                    workbench.update(cx, |workbench, _| {
                        workbench.lens = lens;
                        workbench.offset = 5;
                        workbench.scroll.set_offset(scroll);
                    });
                    // Use the same method as the document tab. No source path
                    // is open here, so switching back must retain its position.
                    shell.resume_systems(window, cx);
                    assert!(shell.systems_active);
                    let workbench = workbench.read(cx);
                    assert!(workbench.lens == lens);
                    assert_eq!(workbench.offset, 5);
                    assert_eq!(workbench.scroll.offset(), scroll);
                    assert!(workbench.focus.is_focused(window));
                });
            });
        }
    }
}

#[gpui::test]
fn shell_binary_export_preserves_hidden_text_and_handles_cancel_and_failure(
    cx: &mut TestAppContext,
) {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("input.bin");
    let output = directory.path().join("copy.bin");
    std::fs::write(&source, fixture().bytes()).unwrap();
    let (shell, cx) = cx.add_window_view(crate::AppShell::new);
    cx.simulate_input("unsaved text");
    let text = text_state(&shell, cx);
    let workbench = load_binary(&shell, source.clone(), cx);
    assert!(!cx.has_pending_prompt());
    assert_eq!(text_state(&shell, cx), text);
    patch(&workbench, "b8 2a 00 00 00", cx);
    let expected = update(&workbench, cx, |workbench, _| {
        workbench.image.bytes().to_vec()
    });
    cx.dispatch_action(crate::Save);
    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();
    update(&workbench, cx, |workbench, _| assert!(workbench.is_dirty()));
    assert_eq!(text_state(&shell, cx), text);
    cx.dispatch_action(crate::Save);
    cx.simulate_new_path_selection(|_| Some(source.clone()));
    cx.run_until_parked();
    update(&workbench, cx, |workbench, _| assert!(workbench.is_dirty()));
    assert_eq!(std::fs::read(&source).unwrap(), fixture().bytes());
    assert_eq!(text_state(&shell, cx), text);
    cx.dispatch_action(crate::Save);
    cx.simulate_new_path_selection(|_| Some(output.clone()));
    cx.run_until_parked();
    assert_eq!(std::fs::read(&output).unwrap(), expected);
    assert_eq!(std::fs::read(&source).unwrap(), fixture().bytes());
    update(&workbench, cx, |workbench, _| {
        assert!(!workbench.is_dirty())
    });
    assert_eq!(text_state(&shell, cx), text);
}

#[gpui::test]
fn shell_close_checks_text_then_binary_and_cancel_retains_both(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let (shell, cx) = cx.add_window_view(crate::AppShell::new);
    cx.simulate_input("keep text");
    let workbench = load_binary(&shell, directory.path().join("fixture.bin"), cx);
    patch(&workbench, "90", cx);
    for action in [false, true] {
        if action {
            cx.dispatch_action(crate::Quit);
        } else {
            cx.dispatch_action(crate::Close);
        }
        assert!(cx.pending_prompt().unwrap().0.contains("Save changes"));
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        assert_eq!(text_state(&shell, cx), ("keep text".into(), true));
        update(&workbench, cx, |workbench, _| assert!(workbench.is_dirty()));
    }
    cx.dispatch_action(crate::Close);
    cx.simulate_prompt_answer("Save");
    cx.run_until_parked();
    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();
    assert_eq!(text_state(&shell, cx), ("keep text".into(), true));
    cx.dispatch_action(crate::Close);
    cx.simulate_prompt_answer("Discard");
    cx.run_until_parked();
    assert!(cx.pending_prompt().unwrap().0.contains("Export binary"));
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert_eq!(text_state(&shell, cx), ("keep text".into(), true));
    update(&workbench, cx, |workbench, _| assert!(workbench.is_dirty()));
    cx.dispatch_action(crate::Close);
    cx.simulate_prompt_answer("Discard");
    cx.run_until_parked();
    cx.simulate_prompt_answer("Export copy");
    cx.run_until_parked();
    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();
    assert_eq!(text_state(&shell, cx), ("keep text".into(), true));
    update(&workbench, cx, |workbench, _| assert!(workbench.is_dirty()));
    // An explicitly discarded old binary can be replaced without losing text.
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.request_binary_replace(
                crate::BinaryIntent::Open(directory.path().join("next.bin"), Box::new(fixture())),
                window,
                cx,
            );
        });
    });
    cx.simulate_prompt_answer("Discard patches");
    cx.run_until_parked();
    assert_eq!(text_state(&shell, cx), ("keep text".into(), true));
    cx.update(|_, cx| {
        let replacement = shell.read(cx).workbench.as_ref().unwrap().read(cx);
        assert_eq!(replacement.path, directory.path().join("next.bin"));
        assert!(!replacement.is_dirty());
    });
}

fn emit_source(
    workbench: &Entity<Workbench>,
    path: &std::path::Path,
    line: u32,
    cx: &mut VisualTestContext,
) {
    update(workbench, cx, |_, cx| {
        cx.emit(WorkbenchEvent::Source(SourceLocation {
            path: path.to_string_lossy().into_owned(),
            line,
            column: 1,
            address: 0,
            end_address: 1,
            file_offset: Some(0),
        }));
    });
    cx.run_until_parked();
}

#[gpui::test]
fn source_navigation_reuses_dirty_matching_buffer_and_guards_replacement(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.c");
    let other = directory.path().join("other.c");
    std::fs::write(&source, "first\nsecond\nthird\n").unwrap();
    std::fs::write(&other, "other\nsource\n").unwrap();
    let (shell, cx) = cx.add_window_view(crate::AppShell::new);
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.apply_intent(
                crate::Intent::Open(Box::new(crate::persistence::load(&source).unwrap())),
                window,
                cx,
            );
        });
    });
    cx.simulate_input("dirty ");
    let before = text_state(&shell, cx);
    let workbench = load_binary(&shell, directory.path().join("fixture.bin"), cx);
    emit_source(&workbench, &source, 3, cx);
    assert!(!cx.has_pending_prompt());
    assert_eq!(text_state(&shell, cx), before);
    cx.update(|_, cx| {
        assert!(!shell.read(cx).systems_active);
        assert_eq!(shell.read(cx).editor.read(cx).cursor().line, 2);
    });
    emit_source(&workbench, &other, 2, cx);
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert_eq!(text_state(&shell, cx), before);
    emit_source(&workbench, &other, 2, cx);
    cx.simulate_prompt_answer("Discard");
    cx.run_until_parked();
    assert_eq!(text_state(&shell, cx), ("other\nsource\n".into(), false));
    cx.update(|_, cx| {
        assert_eq!(shell.read(cx).path.as_ref(), Some(&other));
        assert_eq!(shell.read(cx).editor.read(cx).cursor().line, 1);
        assert!(shell.read(cx).workbench.is_some());
    });
}

#[gpui::test]
fn source_round_trip_retains_exact_instruction_and_interior_byte(cx: &mut TestAppContext) {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        eprintln!("SKIP: exact-location DWARF fixture requires a Linux x86-64 compiler.");
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("mapped.c");
    let binary = directory.path().join("mapped.elf");
    std::fs::write(
        &source,
        "__attribute__((noinline)) int mapped_line(int n) { return (n + 4) ^ 3; }\nint main(void) { return mapped_line(7); }\n",
    ).unwrap();
    let output = match std::process::Command::new("cc")
        .args(["-g", "-O0", "-fno-inline", "-fno-pie", "-no-pie"])
        .arg(&source)
        .arg("-o")
        .arg(&binary)
        .env("LC_ALL", "C")
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!(
                "SKIP: optional cc is unavailable; exact-location source/assembly acceptance remains unverified."
            );
            return;
        }
        Err(error) => panic!("could not compile controlled DWARF fixture: {error}"),
    };
    assert!(
        output.status.success(),
        "fixture compilation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Compile and inspect only; the generated executable is never launched.
    let image = BinaryImage::parse(std::fs::read(&binary).unwrap()).unwrap();
    let symbol = image
        .symbols
        .iter()
        .find(|symbol| symbol.name == "mapped_line")
        .unwrap();
    let instructions = image
        .disassemble(symbol.file_offset.unwrap(), symbol.size as usize, 128)
        .unwrap();
    let first_for_line = image
        .source_locations
        .iter()
        .find(|location| std::path::Path::new(&location.path) == source && location.line == 1)
        .and_then(|location| location.file_offset)
        .unwrap();
    let instruction = instructions
        .iter()
        .skip(1)
        .find(|instruction| {
            instruction.bytes.len() > 1
                && instruction.offset > first_for_line
                && [instruction.offset, instruction.offset + 1]
                    .into_iter()
                    .all(|offset| {
                        image.source_at_offset(offset).is_some_and(|location| {
                            std::path::Path::new(&location.path) == source && location.line == 1
                        })
                    })
        })
        .expect("source line must map to multiple instructions and an interior byte");
    let selected_offsets = [instruction.offset, instruction.offset + 1];
    let (shell, cx) = cx.add_window_view(crate::AppShell::new);
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.request_binary_replace(
                crate::BinaryIntent::Open(binary, Box::new(image)),
                window,
                cx,
            );
        });
    });
    cx.run_until_parked();
    let workbench = cx.update(|_, cx| shell.read(cx).workbench.clone().unwrap());
    update(&workbench, cx, |workbench, cx| {
        assert!(
            workbench.analysis.result.is_some(),
            "{}",
            workbench.analysis.status
        );
        workbench.go(selected_offsets[0], cx);
        assert_eq!(workbench.selected_function().unwrap().name, "mapped_line");
        workbench.refresh_analysis(cx);
        assert!(workbench.analysis.result.is_none());
        workbench.cancel_analysis(cx);
    });
    cx.run_until_parked();
    update(&workbench, cx, |workbench, cx| {
        assert!(
            workbench.analysis.result.is_none(),
            "Cancelled completion must not publish"
        );
        assert!(!workbench.analysis.running);
        workbench.refresh_analysis(cx);
    });
    cx.run_until_parked();
    update(&workbench, cx, |workbench, _| {
        assert!(workbench.analysis.result.is_some())
    });
    for offset in selected_offsets {
        let history = update(&workbench, cx, |workbench, cx| {
            workbench.go(offset, cx);
            let history = workbench.back.clone();
            // Use the real Source command and shell event subscriber.
            workbench.source(cx);
            history
        });
        cx.run_until_parked();
        assert!(!cx.has_pending_prompt());
        cx.update(|_, cx| {
            let shell = shell.read(cx);
            assert!(!shell.systems_active);
            assert_eq!(shell.path.as_ref(), Some(&source));
            assert_eq!(shell.editor.read(cx).cursor().line, 0);
        });
        cx.dispatch_action(crate::ShowAssembly);
        cx.run_until_parked();
        update(&workbench, cx, |workbench, _| {
            assert!(workbench.lens == Lens::Assembly);
            assert_eq!(
                workbench.offset, offset,
                "source round trip must not jump to the first instruction on a shared source line"
            );
            assert_ne!(workbench.offset, first_for_line);
            assert_eq!(
                workbench.back, history,
                "representation changes must not create spurious navigation history"
            );
        });
        update(&workbench, cx, |workbench, cx| {
            workbench.lens = Lens::Bytes;
            workbench.source(cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            shell.update(cx, |shell, cx| shell.resume_systems(window, cx));
            let workbench = workbench.read(cx);
            assert!(workbench.lens == Lens::Bytes);
            assert_eq!(workbench.offset, offset);
            assert_eq!(workbench.back, history);
        });
    }
    let before = update(&workbench, cx, |workbench, cx| {
        workbench.go(instructions[0].offset, cx);
        workbench.analysis.result.clone().unwrap()
    });
    patch(&workbench, "90", cx);
    update(&workbench, cx, |workbench, cx| {
        let after = workbench.analysis.result.as_ref().unwrap();
        assert!(
            !Arc::ptr_eq(&before, after),
            "Patch must replace analysis snapshot"
        );
        assert_eq!(
            workbench.selected_function().unwrap().blocks[0].instructions[0].bytes,
            [0x90]
        );
        workbench.history(false, cx);
    });
    cx.run_until_parked();
    update(&workbench, cx, |workbench, _| {
        assert_eq!(
            workbench.selected_function().unwrap().blocks[0].instructions[0].bytes,
            instructions[0].bytes
        );
        assert!(!workbench.is_dirty());
    });
}

#[gpui::test]
fn edits_during_binary_export_require_another_close_decision(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("snapshot.bin");
    let (shell, cx) = cx.add_window_view(crate::AppShell::new);
    let workbench = load_binary(&shell, directory.path().join("fixture.bin"), cx);
    patch(&workbench, "b8 2a 00 00 00", cx);
    let captured = update(&workbench, cx, |workbench, _| {
        workbench.image.bytes().to_vec()
    });
    cx.dispatch_action(crate::Close);
    cx.simulate_prompt_answer("Export copy");
    cx.run_until_parked();
    cx.simulate_new_path_selection(|_| Some(output.clone()));
    // Advance one executor task at a time to stop after the snapshot is captured
    // and before completion. This exercises actual shell picker/export wiring.
    for _ in 0..1000 {
        let captured = cx.update(|_, cx| shell.read(cx).status.starts_with("Exporting captured"));
        if captured {
            break;
        }
        assert!(
            cx.executor().tick(),
            "export stalled before capturing a snapshot"
        );
    }
    cx.update(|window, cx| {
        assert!(shell.read(cx).status.starts_with("Exporting captured"));
        let editor = shell.read(cx).editor.clone();
        editor.update(cx, |editor, cx| {
            editor.replace_text_in_range(None, "new text", window, cx)
        });
        // Undo is a real, synchronous patch edit while the export is in flight.
        workbench.update(cx, |workbench, cx| workbench.history(false, cx));
    });
    cx.run_until_parked();
    assert_eq!(std::fs::read(output).unwrap(), captured);
    assert_eq!(text_state(&shell, cx), ("new text".into(), true));
    assert!(cx.pending_prompt().unwrap().0.contains("Save changes"));
    cx.simulate_prompt_answer("Discard");
    cx.run_until_parked();
    assert!(cx.pending_prompt().unwrap().0.contains("Export binary"));
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    update(&workbench, cx, |workbench, _| {
        assert_eq!(workbench.image.bytes(), fixture().bytes());
        assert!(workbench.is_dirty());
    });
}

#[gpui::test]
fn hidden_text_edit_during_binary_close_prompt_requires_new_text_decision(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let (shell, cx) = cx.add_window_view(crate::AppShell::new);
    cx.simulate_input("original text");
    let workbench = load_binary(&shell, directory.path().join("fixture.bin"), cx);
    patch(&workbench, "90", cx);
    cx.dispatch_action(crate::Close);
    cx.simulate_prompt_answer("Discard");
    cx.run_until_parked();
    assert!(cx.pending_prompt().unwrap().0.contains("Export binary"));
    cx.update(|window, cx| {
        let editor = shell.read(cx).editor.clone();
        editor.update(cx, |editor, cx| {
            editor.replace_text_in_range(None, " changed while deciding", window, cx);
        });
    });
    cx.simulate_prompt_answer("Discard patches");
    cx.run_until_parked();
    assert!(cx.pending_prompt().unwrap().0.contains("Save changes"));
    assert_eq!(
        text_state(&shell, cx),
        ("original text changed while deciding".into(), true)
    );
    update(&workbench, cx, |workbench, _| {
        assert_eq!(workbench.image.bytes()[0], 0x90);
        assert!(workbench.is_dirty());
    });
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    cx.update(|_, cx| {
        let shell = shell.read(cx);
        assert!(shell.close_text_revision.is_none());
        assert!(shell.close_binary_revision.is_none());
        assert!(shell.workbench.is_some());
    });
    assert_eq!(
        text_state(&shell, cx),
        ("original text changed while deciding".into(), true)
    );
}

#[gpui::test]
fn unchanged_text_discard_then_binary_export_does_not_repeat_text_prompt(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("approved-copy.bin");
    let (shell, cx) = cx.add_window_view(crate::AppShell::new);
    cx.simulate_input("explicitly discarded text");
    let workbench = load_binary(&shell, directory.path().join("fixture.bin"), cx);
    patch(&workbench, "90", cx);
    let expected = update(&workbench, cx, |workbench, _| {
        workbench.image.bytes().to_vec()
    });
    cx.dispatch_action(crate::Close);
    cx.simulate_prompt_answer("Discard");
    cx.run_until_parked();
    assert!(cx.pending_prompt().unwrap().0.contains("Export binary"));
    cx.simulate_prompt_answer("Export copy");
    cx.run_until_parked();
    cx.simulate_new_path_selection(|_| Some(output.clone()));
    cx.run_until_parked();
    assert_eq!(std::fs::read(output).unwrap(), expected);
    assert!(
        !cx.has_pending_prompt(),
        "The same text revision was already explicitly discarded for this close."
    );
    update(&workbench, cx, |workbench, _| {
        assert!(!workbench.is_dirty())
    });
    cx.update(|_, cx| {
        let shell = shell.read(cx);
        assert!(!shell.busy);
        assert!(shell.binary_pending.is_none());
        assert_eq!(
            shell.close_text_revision,
            Some(shell.editor.read(cx).content_revision())
        );
    });
    // GPUI's TestPlatform::quit is a no-op: this verifies the prompt/approval
    // state machine, while normal process exit still needs native acceptance.
}

#[gpui::test]
fn binary_close_approval_is_cleared_on_abort_and_scoped_to_its_document(cx: &mut TestAppContext) {
    let (shell, cx) = cx.add_window_view(crate::AppShell::new);
    let first = load_binary(&shell, PathBuf::from("first.bin"), cx);
    patch(&first, "90", cx);
    let first_revision = update(&first, cx, |workbench, _| workbench.snapshot().1);
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.close_text_revision = Some(shell.editor.read(cx).content_revision());
            shell.close_binary_revision = Some((first.entity_id(), first_revision));
            first.update(cx, |workbench, _| workbench.busy = true);
            shell.request_binary_replace(crate::BinaryIntent::Close, window, cx);
            assert!(shell.close_text_revision.is_none());
            assert!(shell.close_binary_revision.is_none());
            assert!(
                shell
                    .status
                    .contains("Wait for the current binary operation")
            );
            first.update(cx, |workbench, _| workbench.busy = false);
        });
    });
    assert!(!cx.has_pending_prompt());
    // An approval for Close must also not silently authorize replacing a binary.
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.close_text_revision = Some(shell.editor.read(cx).content_revision());
            shell.close_binary_revision = Some((first.entity_id(), first_revision));
            shell.request_binary_replace(
                crate::BinaryIntent::Open(PathBuf::from("second.bin"), Box::new(fixture())),
                window,
                cx,
            );
            assert!(shell.close_text_revision.is_none());
            assert!(shell.close_binary_revision.is_none());
        });
    });
    assert!(cx.pending_prompt().unwrap().0.contains("Export binary"));
    cx.simulate_prompt_answer("Discard patches");
    cx.run_until_parked();
    let second = cx.update(|_, cx| shell.read(cx).workbench.clone().unwrap());
    assert_ne!(first.entity_id(), second.entity_id());
    patch(&second, "91", cx);
    let second_revision = update(&second, cx, |workbench, _| workbench.snapshot().1);
    assert_eq!(
        first_revision, second_revision,
        "fixture must reproduce revision-number reuse across documents"
    );
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.close_binary_revision = Some((first.entity_id(), first_revision));
            shell.request_binary_replace(crate::BinaryIntent::Close, window, cx);
        });
    });
    assert!(
        cx.pending_prompt().unwrap().0.contains("Export binary"),
        "A different document with the same revision number must require its own decision."
    );
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    update(&second, cx, |workbench, _| {
        assert_eq!(workbench.image.bytes()[0], 0x91);
        assert!(workbench.is_dirty());
    });
}

#[gpui::test]
fn native_window_close_defers_clean_quit_past_the_platform_callback(cx: &mut TestAppContext) {
    let (shell, cx) = cx.add_window_view(crate::AppShell::new);
    assert!(!cx.simulate_close());
    cx.update(|_, cx| {
        assert!(shell.read(cx).close_text_revision.is_none(), "Close handling must not run while the platform close callback is still on the stack; App::defer is too early.");
    });
    assert!(!cx.has_pending_prompt());
    cx.run_until_parked();
    cx.update(|_, cx| {
        let shell = shell.read(cx);
        assert_eq!(
            shell.close_text_revision,
            Some(shell.editor.read(cx).content_revision())
        );
        assert!(!shell.busy);
    });
    assert!(!cx.has_pending_prompt());
    // The test platform does not implement X11 borrowing or real process quit.
    // Native WM_DELETE_WINDOW / Alt+F4 must additionally exit normally.
}

#[gpui::test]
fn native_window_close_still_guards_dirty_text_and_binary_after_deferral(cx: &mut TestAppContext) {
    let (shell, cx) = cx.add_window_view(crate::AppShell::new);
    cx.simulate_input("retain text");
    let workbench = load_binary(&shell, PathBuf::from("native-close.bin"), cx);
    patch(&workbench, "90", cx);
    assert!(!cx.simulate_close());
    assert!(!cx.has_pending_prompt());
    cx.run_until_parked();
    assert!(cx.pending_prompt().unwrap().0.contains("Save changes"));
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert_eq!(text_state(&shell, cx), ("retain text".into(), true));
    update(&workbench, cx, |workbench, _| assert!(workbench.is_dirty()));
    assert!(!cx.simulate_close());
    cx.run_until_parked();
    cx.simulate_prompt_answer("Discard");
    cx.run_until_parked();
    assert!(cx.pending_prompt().unwrap().0.contains("Export binary"));
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert_eq!(text_state(&shell, cx), ("retain text".into(), true));
    update(&workbench, cx, |workbench, _| {
        assert!(workbench.is_dirty());
        assert_eq!(workbench.image.bytes()[0], 0x90);
    });
}
