//! Native replacement compilation is bound to binary and source snapshots.
use super::*;
use ale_systems_core::recompile::ReplacementSite;
use std::sync::atomic::{AtomicBool, Ordering};
#[derive(Default)]
pub(super) struct Compilation {
    pub generation: u64,
    pub running: bool,
    pub cancel: Option<Arc<AtomicBool>>,
    pub source: Option<(Entity<EditorView>, u64)>,
    pub watch: Option<gpui::Subscription>,
}
impl Drop for Compilation {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}
impl Workbench {
    pub(crate) fn invalidate_compilation(&mut self) {
        if let Some(cancel) = self.compilation.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.compilation.generation = self.compilation.generation.wrapping_add(1);
        self.compilation.running = false;
        self.compilation.source = None;
        self.compilation.watch = None;
        self.preview = None;
        self.preview_body_visible = false;
    }
    pub(super) fn edit_recovered_c(&mut self, cx: &mut Context<Self>) {
        let Some(result) = &self.decompiler.result else {
            self.message = "Decompile a function first.".into();
            cx.notify();
            return;
        };
        if self.busy {
            return;
        }
        let image = self.image.clone();
        let entry = result.entry;
        let text = result.text.clone();
        let revision = self.target_epoch;
        self.busy = true;
        let work = cx
            .background_executor()
            .spawn(async move { ReplacementSite::discover(&image, entry) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                if this.target_epoch != revision {
                    return;
                }
                match result {
                    Ok(site) => cx.emit(WorkbenchEvent::RecoveredC {
                        site,
                        text,
                        revision,
                    }),
                    Err(error) => this.message = error,
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub(crate) fn relocated_mode(&self) -> bool {
        self.placement_mode == PlacementMode::Relocated
    }
    pub(crate) fn toggle_relocated_mode(&mut self, cx: &mut Context<Self>) {
        self.invalidate_compilation();
        self.placement_mode = if self.placement_mode == PlacementMode::FixedSpan {
            PlacementMode::Relocated
        } else {
            PlacementMode::FixedSpan
        };
        self.message=if self.relocated_mode() {"Relocated mode: supported x86-64 Linux executables only. No external imports, TLS or new mutable storage. ABI, unwinding and unknown interior entries are not proven."}else{"Fixed-span mode: replacement must fit the original function."}.into();
        cx.notify();
    }
    pub(crate) fn compile_replacement(
        &mut self,
        site: ReplacementSite,
        source: Entity<EditorView>,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        self.invalidate_compilation();
        let text = source.read(cx).text();
        let source_revision = source.read(cx).content_revision();
        let generation = self.compilation.generation;
        let revision = self.revision;
        let mode = self.placement_mode;
        let cancel = Arc::new(AtomicBool::new(false));
        self.compilation.cancel = Some(cancel.clone());
        self.compilation.running = true;
        self.compilation.source = Some((source.clone(), source_revision));
        self.compilation.watch = Some(cx.observe(&source, move |this, editor, cx| {
            if editor.read(cx).content_revision() != source_revision {
                this.invalidate_compilation();
                this.message = "C source changed; compile a new preview.".into();
                cx.notify();
            }
        }));
        self.message = format!(
            "Compiling {} ({})…",
            site.name,
            if mode == PlacementMode::Relocated {
                "relocated"
            } else {
                "fixed span"
            }
        );
        let job_site = site.clone();
        let image = self.image.clone();
        let work = cx.background_executor().spawn(async move {
            let profile = if mode == PlacementMode::Relocated {
                crate::recompiler::CompileProfile::RelocatablePic
            } else {
                crate::recompiler::CompileProfile::LegacyLeaf
            };
            let object = crate::recompiler::compile_object(&text, profile, &cancel)?;
            let limits = ale_systems_core::rebuild::RebuildLimits::default();
            let plan = if mode == PlacementMode::FixedSpan {
                let bytes = job_site.replacement(&object.bytes)?;
                ale_systems_core::rebuild::ReplacementPlan::from_fixed(&image, &job_site, bytes)
                    .map_err(|e| e.to_string())?
            } else {
                let unit = ale_systems_core::rebuild::RelocatableUnit::parse(
                    &object.bytes,
                    &job_site.name,
                    &limits,
                    &cancel,
                )
                .map_err(|e| e.to_string())?;
                ale_systems_core::rebuild::plan_replacement(
                    &image,
                    &job_site,
                    &unit,
                    ale_systems_core::rebuild::RebuildOptions { mode, limits },
                    &cancel,
                )
                .map_err(|e| e.to_string())?
            };
            Ok::<_, String>((Arc::new(plan), object.compiler_identity))
        });
        cx.spawn(async move |this,cx| {
            let result=work.await;
            let _=this.update(cx,|this,cx| {
                if this.compilation.generation!=generation || this.revision!=revision || this.placement_mode!=mode {return;}
                this.compilation.running=false;this.compilation.cancel=None;
                if source.read(cx).content_revision()!=source_revision {this.invalidate_compilation();this.message="C source changed; compile again.".into();cx.notify();return;}
                match result {
                    Ok((plan,compiler_identity))=>{
                        let histories=this.undo.iter().chain(&this.redo).fold(0usize,|n,e|n.saturating_add(e.image.bytes().len()));
                        let required=histories.saturating_add(this.image.bytes().len()).saturating_add(plan.candidate().bytes().len());
                        if required>128*1024*1024 {this.invalidate_compilation();this.message="Preview would exceed the 128 MiB binary history limit; export or reduce the image before compiling.".into();}
                        else {
                            this.offset=site.offset;this.decode();this.lens=Lens::Assembly;this.preview_body_visible=false;
                            this.preview=Some(Preview::Native {plan,binary_revision:revision,compiler_identity,guard:NativeGuard {owner:cx.entity_id(),generation,source:source.entity_id(),source_revision}});
                            this.message=format!("Compiled C preview for {}. Review the placement and ABI limits, then Apply and Export copy.",site.name);
                        }
                    },
                    Err(error)=>this.message=error,
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }
}

#[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
mod tests {
    use super::*;
    use gpui::{EntityInputHandler, TestAppContext};
    #[gpui::test]
    fn source_undo_redo_and_entity_switch_invalidate_only_the_right_preview(
        cx: &mut TestAppContext,
    ) {
        let (_dir, _path, image, site) = crate::recompiler::tests::fixture();
        let before = image.bytes().to_vec();
        let (source, cx) =
            cx.add_window_view(|_, cx| EditorView::new("int roundtrip(int x){return x+7;}", cx));
        cx.update(|window, cx| source.read(cx).focus_handle(cx).focus(window));
        let workbench =
            cx.update(|_, cx| cx.new(|cx| Workbench::new(PathBuf::from("fixture.elf"), image, cx)));
        cx.run_until_parked();
        cx.update(|_, cx| {
            workbench.update(cx, |view, cx| {
                view.compile_replacement(site.clone(), source.clone(), cx)
            })
        });
        cx.run_until_parked();
        assert!(cx.update(|_, cx| workbench.read(cx).preview.is_some()));
        cx.simulate_input(" ");
        assert!(cx.update(|_, cx| source.read(cx).text().starts_with(' ')));
        cx.simulate_keystrokes("ctrl-z");
        assert_eq!(
            cx.update(|_, cx| source.read(cx).text()),
            "int roundtrip(int x){return x+7;}"
        );
        cx.run_until_parked();
        cx.update(|_, cx| {
            workbench.update(cx, |view, cx| {
                view.apply_patch(cx);
                assert!(view.preview.is_none());
                assert_eq!(view.image.bytes(), before);
            })
        });
        cx.update(|_, cx| {
            workbench.update(cx, |view, cx| {
                view.compile_replacement(site.clone(), source.clone(), cx)
            })
        });
        cx.run_until_parked();
        assert!(cx.update(|_, cx| workbench.read(cx).preview.is_some()));
        cx.simulate_keystrokes("ctrl-y");
        assert!(cx.update(|_, cx| source.read(cx).text().starts_with(' ')));
        cx.run_until_parked();
        assert!(cx.update(|_, cx| workbench.read(cx).preview.is_none()));
        let other = cx
            .update(|_, cx| cx.new(|cx| EditorView::new("int roundtrip(int x){return x+7;}", cx)));
        cx.update(|_, cx| {
            workbench.update(cx, |view, cx| {
                view.compile_replacement(site, other.clone(), cx)
            })
        });
        cx.run_until_parked();
        assert!(cx.update(|_, cx| workbench.read(cx).preview.is_some()));
        cx.update(|window, cx| {
            source.update(cx, |source, cx| {
                source.replace_text_in_range(Some(0..0), " ", window, cx)
            })
        });
        cx.run_until_parked();
        assert!(cx.update(|_, cx| workbench.read(cx).preview.is_some()));
        cx.update(|window, cx| {
            other.update(cx, |source, cx| {
                source.replace_text_in_range(Some(0..0), " ", window, cx)
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            workbench.update(cx, |view, cx| {
                view.apply_patch(cx);
                assert!(view.preview.is_none());
                assert_eq!(view.image.bytes(), before);
            })
        });
    }
    #[gpui::test]
    fn edited_source_cannot_apply_stale_compilation(cx: &mut TestAppContext) {
        let (_dir, _path, image, site) = crate::recompiler::tests::fixture();
        let before = image.bytes().to_vec();
        let (view, cx) =
            cx.add_window_view(|_, cx| Workbench::new(PathBuf::from("fixture.elf"), image, cx));
        cx.run_until_parked();
        let source = cx
            .update(|_, cx| cx.new(|cx| EditorView::new("int roundtrip(int x){return x+7;}", cx)));
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.compile_replacement(site.clone(), source.clone(), cx)
            })
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(view.read(cx).preview.is_some(), "{}", view.read(cx).message);
            source.update(cx, |source, cx| {
                source.replace_text_in_range(Some(0..0), " ", window, cx)
            });
            view.update(cx, |view, cx| {
                view.apply_patch(cx);
                assert!(view.preview.is_none());
                assert_eq!(view.image.bytes(), before);
                assert_eq!(view.revision, 0);
            });
        });
        cx.run_until_parked();
        cx.update(|_, cx| view.update(cx, |view, cx| view.compile_replacement(site, source, cx)));
        cx.run_until_parked();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                assert!(view.preview.is_some(), "{}", view.message);
                view.apply_patch(cx);
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            view.update(cx, |view, _| {
                assert_eq!(view.revision, 1);
                assert!(view.executable_export);
                assert_ne!(view.image.bytes(), before);
            })
        });
    }
    #[gpui::test]
    fn cancelled_and_invalidated_binary_jobs_never_publish(cx: &mut TestAppContext) {
        let (_dir, _path, image, site) = crate::recompiler::tests::fixture();
        let (view, cx) =
            cx.add_window_view(|_, cx| Workbench::new(PathBuf::from("fixture.elf"), image, cx));
        cx.run_until_parked();
        let source = cx
            .update(|_, cx| cx.new(|cx| EditorView::new("int roundtrip(int x){return x+7;}", cx)));
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.compile_replacement(site.clone(), source.clone(), cx);
                view.invalidate_compilation();
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                assert!(view.preview.is_none());
                assert!(!view.compilation.running);
                view.compile_replacement(site, source, cx);
                view.revision += 1;
                view.refresh_analysis(cx);
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| view.update(cx, |view, _| assert!(view.preview.is_none())));
    }
}

#[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
mod relocated_tests {
    use super::*;
    use gpui::{EntityInputHandler, TestAppContext};
    use std::{fs, process::Command};
    fn fixture() -> (tempfile::TempDir, PathBuf, BinaryImage, ReplacementSite) {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("fixture.c");
        let binary = dir.path().join("fixture.elf");
        fs::write(&source,"__attribute__((noinline)) int roundtrip(int x){return x*x+17;} int main(void){return roundtrip(4);}\n").unwrap();
        let out = Command::new("/usr/bin/cc")
            .args([
                "-O1",
                "-no-pie",
                "-fno-asynchronous-unwind-tables",
                "-fno-unwind-tables",
                "-fcf-protection=none",
            ])
            .arg(&source)
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let image = BinaryImage::parse(fs::read(&binary).unwrap()).unwrap();
        let address = image
            .symbols
            .iter()
            .find(|s| s.name == "roundtrip")
            .unwrap()
            .address;
        let site = ReplacementSite::discover(&image, address).unwrap();
        (dir, binary, image, site)
    }
    fn source(cx: &mut TestAppContext) -> Entity<EditorView> {
        cx.update(|cx| {
            cx.new(|cx| {
                EditorView::new(
                    "int roundtrip(int x){volatile int v=x*7+100;return (v^53)+(v>>2);}",
                    cx,
                )
            })
        })
    }
    #[gpui::test]
    fn native_preview_displays_whole_plan_and_applies_one_undo_step(cx: &mut TestAppContext) {
        let (_dir, path, image, site) = fixture();
        let before = image.bytes().to_vec();
        let (view, cx) = cx.add_window_view(|_, cx| Workbench::new(path, image, cx));
        let source = source(cx);
        cx.run_until_parked();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.toggle_relocated_mode(cx);
                view.compile_replacement(site, source, cx);
            })
        });
        cx.run_until_parked();
        let candidate = cx.update(|_, cx| {
            let view = view.read(cx);
            let Some(Preview::Native {
                plan,
                compiler_identity,
                ..
            }) = &view.preview
            else {
                panic!("{}", view.message)
            };
            let text = native_preview_text(plan, compiler_identity);
            assert!(
                text.contains("Relocated")
                    && text.contains("entry")
                    && text.contains("body")
                    && text.contains("changed original ranges")
                    && text.contains("relocations")
            );
            assert!(!native_body_text(plan).is_empty());
            assert!(plan.summary().body_span.len() > plan.summary().original_span.len());
            plan.candidate().bytes().to_vec()
        });
        cx.update(|_, cx| {
            view.update(cx, |view, _| {
                view.back.push(1);
                view.matches.push((1, "old".into()));
                view.decompiler.result = Some(crate::decompiler::Decompiled {
                    entry: 0,
                    text: "old".into(),
                    tokens: vec![],
                    diagnostics: vec![],
                });
            })
        });
        cx.update(|_, cx| view.update(cx, |view, cx| view.apply_patch(cx)));
        cx.run_until_parked();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                assert!(
                    view.back.is_empty()
                        && view.matches.is_empty()
                        && view.decompiler.result.is_none()
                );
                let body = view.image.rewrite_locations()[0].body_address;
                assert!(
                    view.analysis
                        .result
                        .as_ref()
                        .unwrap()
                        .functions
                        .iter()
                        .any(|f| f.address == body)
                );
                assert!(view.analysis.calls.is_some());
                assert!(!view.entropy.is_empty());
                assert_eq!(view.image.bytes(), candidate);
                assert!(view.executable_export);
                assert_eq!(view.revision, 1);
                view.history(false, cx);
                assert_eq!(view.image.bytes(), before);
                assert!(!view.executable_export);
                view.history(true, cx);
                assert_eq!(view.image.bytes(), candidate);
                assert!(view.executable_export);
            })
        });
    }
    #[gpui::test]
    fn mode_change_and_cancellation_discard_late_results(cx: &mut TestAppContext) {
        let (_dir, path, image, site) = fixture();
        let before = image.bytes().to_vec();
        let (view, cx) = cx.add_window_view(|_, cx| Workbench::new(path, image, cx));
        let source = source(cx);
        cx.run_until_parked();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.toggle_relocated_mode(cx);
                view.compile_replacement(site.clone(), source.clone(), cx);
                view.toggle_relocated_mode(cx);
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                assert!(view.preview.is_none());
                assert_eq!(view.image.bytes(), before);
                view.toggle_relocated_mode(cx);
                view.compile_replacement(site, source, cx);
                view.invalidate_compilation();
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert!(view.read(cx).preview.is_none());
            assert_eq!(view.read(cx).image.bytes(), before);
        });
    }
    #[gpui::test]
    fn relocated_source_edit_invalidates_preview(cx: &mut TestAppContext) {
        let (_dir, path, image, site) = fixture();
        let before = image.bytes().to_vec();
        let (view, cx) = cx.add_window_view(|_, cx| Workbench::new(path, image, cx));
        let source = source(cx);
        cx.run_until_parked();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.toggle_relocated_mode(cx);
                view.compile_replacement(site, source.clone(), cx);
            })
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(view.read(cx).preview.is_some());
            source.update(cx, |source, cx| {
                source.replace_text_in_range(Some(0..0), " ", window, cx)
            });
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.apply_patch(cx);
                assert!(view.preview.is_none());
                assert_eq!(view.image.bytes(), before);
            })
        });
    }
    #[gpui::test]
    fn late_plan_and_apply_are_rejected_after_source_edit(cx: &mut TestAppContext) {
        let (_dir, path, image, site) = fixture();
        let before = image.bytes().to_vec();
        let (view, cx) = cx.add_window_view(|_, cx| Workbench::new(path, image, cx));
        let source = source(cx);
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.toggle_relocated_mode(cx);
                view.compile_replacement(site.clone(), source.clone(), cx);
            });
            source.update(cx, |source, cx| {
                source.replace_text_in_range(Some(0..0), " ", window, cx)
            });
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert!(view.read(cx).preview.is_none());
            assert_eq!(view.read(cx).image.bytes(), before);
        });
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.compile_replacement(site, source.clone(), cx)
            })
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(view.read(cx).preview.is_some(), "{}", view.read(cx).message);
            view.update(cx, |view, cx| view.apply_patch(cx));
            source.update(cx, |source, cx| {
                source.replace_text_in_range(Some(0..0), " ", window, cx)
            });
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert_eq!(view.read(cx).image.bytes(), before);
            assert_eq!(view.read(cx).revision, 0);
        });
    }
    #[gpui::test]
    fn wrong_owner_generation_revision_source_or_base_rejects_native_plan(cx: &mut TestAppContext) {
        let (_dir, path, image, site) = fixture();
        let original = image.clone();
        let before = image.bytes().to_vec();
        let (view, cx) = cx.add_window_view(|_, cx| Workbench::new(path.clone(), image, cx));
        let source = source(cx);
        let other = cx.update(|_, cx| cx.new(|cx| Workbench::new(path, original.clone(), cx)));
        let other_source =
            cx.update(|_, cx| cx.new(|cx| EditorView::new("int roundtrip(int x){return x;}", cx)));
        cx.run_until_parked();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.toggle_relocated_mode(cx);
                view.compile_replacement(site, source.clone(), cx);
            })
        });
        cx.run_until_parked();
        let valid = cx.update(|_, cx| view.read(cx).preview.clone().expect("native preview"));
        for variation in 0..4 {
            let mut bad = valid.clone();
            if let Preview::Native {
                binary_revision,
                guard,
                ..
            } = &mut bad
            {
                match variation {
                    0 => guard.owner = other.entity_id(),
                    1 => guard.generation = guard.generation.wrapping_add(1),
                    2 => *binary_revision += 1,
                    3 => guard.source = other_source.entity_id(),
                    _ => unreachable!(),
                }
            } else {
                panic!("expected native preview");
            }
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.preview = Some(bad);
                    view.apply_patch(cx);
                    assert_eq!(view.image.bytes(), before);
                    assert!(view.undo.is_empty());
                })
            });
        }
        let comment = original
            .sections
            .iter()
            .find(|s| s.name == ".comment")
            .unwrap()
            .file_offset;
        let changed = original
            .patched(comment, &[original.bytes()[comment] ^ 1])
            .unwrap();
        let changed_bytes = changed.bytes().to_vec();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.image = Arc::new(changed);
                view.preview = Some(valid);
                view.apply_patch(cx);
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let view = view.read(cx);
            assert_eq!(view.image.bytes(), changed_bytes);
            assert_eq!(view.revision, 0);
            assert!(view.undo.is_empty());
        });
    }
    #[gpui::test]
    fn export_reopen_and_body_navigation_keep_complete_candidate(cx: &mut TestAppContext) {
        use std::os::unix::fs::PermissionsExt;
        let (dir, path, image, site) = fixture();
        let original = image.bytes().to_vec();
        let output = dir.path().join("relocated-copy.elf");
        let (shell, cx) = cx.add_window_view(crate::AppShell::new);
        let view = cx.update(|_, cx| {
            shell.update(cx, |shell, cx| {
                let view = cx.new(|cx| Workbench::new(path.clone(), image, cx));
                shell.workbench = Some(view.clone());
                shell.systems_active = true;
                view
            })
        });
        let source = source(cx);
        cx.run_until_parked();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.toggle_relocated_mode(cx);
                view.compile_replacement(site.clone(), source.clone(), cx);
            })
        });
        cx.run_until_parked();
        let expected = cx.update(|_, cx| match &view.read(cx).preview {
            Some(Preview::Native { plan, .. }) => plan.candidate().bytes().to_vec(),
            _ => panic!("{}", view.read(cx).message),
        });
        cx.update(|_, cx| view.update(cx, |view, cx| view.apply_patch(cx)));
        cx.run_until_parked();
        cx.dispatch_action(crate::Save);
        cx.simulate_new_path_selection(|_| Some(path.clone()));
        cx.run_until_parked();
        assert_eq!(fs::read(&path).unwrap(), original);
        cx.dispatch_action(crate::Save);
        cx.simulate_new_path_selection(|_| Some(output.clone()));
        cx.run_until_parked();
        assert_eq!(fs::read(&output).unwrap(), expected);
        assert_eq!(
            fs::metadata(&output).unwrap().permissions().mode() & 0o7777,
            0o700
        );
        cx.update(|_, cx| assert!(!view.read(cx).is_dirty()));
        let reopened = BinaryImage::parse(fs::read(&output).unwrap()).unwrap();
        let body = reopened.rewrite_locations()[0].body_address;
        let reopened = cx.update(|_, cx| cx.new(|cx| Workbench::new(output, reopened, cx)));
        cx.run_until_parked();
        cx.update(|_, cx| {
            reopened.update(cx, |view, cx| {
                assert!(view.executable_export);
                view.go_rewrite_body(cx);
                assert_eq!(view.image.offset_to_address(view.offset), Some(body));
                assert_eq!(view.selected_function().map(|f| f.address), Some(body));
                if std::env::var_os("ALE_DECOMPILER").is_some() {
                    view.start_decompiler(cx);
                }
            })
        });
        cx.run_until_parked();
        if std::env::var_os("ALE_DECOMPILER").is_some() {
            cx.update(|_, cx| {
                let view = reopened.read(cx);
                let result = view
                    .decompiler
                    .result
                    .as_ref()
                    .expect("decompiled replacement body");
                assert_eq!(result.entry, body);
                assert!(result.text.contains("__ale_body_roundtrip"));
            });
        }
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.compile_replacement(site, source.clone(), cx)
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let view = view.read(cx);
            assert!(view.preview.is_none());
            assert!(view.message.contains("previous relocated rewrite"));
            assert_eq!(view.image.bytes(), expected);
        });
    }
    #[gpui::test]
    fn export_captures_native_candidate_when_later_undo_occurs(cx: &mut TestAppContext) {
        use std::os::unix::fs::PermissionsExt;
        let (dir, path, image, site) = fixture();
        let original = image.bytes().to_vec();
        let output = dir.path().join("captured.elf");
        let (shell, cx) = cx.add_window_view(crate::AppShell::new);
        let view = cx.update(|_, cx| {
            shell.update(cx, |shell, cx| {
                let view = cx.new(|cx| Workbench::new(path.clone(), image, cx));
                shell.workbench = Some(view.clone());
                shell.systems_active = true;
                view
            })
        });
        let source = source(cx);
        cx.run_until_parked();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.toggle_relocated_mode(cx);
                view.compile_replacement(site, source, cx);
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| view.update(cx, |view, cx| view.apply_patch(cx)));
        cx.run_until_parked();
        let captured = cx.update(|_, cx| view.read(cx).image.bytes().to_vec());
        assert_ne!(captured, original);
        cx.dispatch_action(crate::Save);
        cx.simulate_new_path_selection(|_| Some(output.clone()));
        for _ in 0..1000 {
            if cx.update(|_, cx| shell.read(cx).status.starts_with("Exporting captured")) {
                break;
            }
            assert!(cx.executor().tick(), "export snapshot was not captured");
        }
        cx.update(|_, cx| view.update(cx, |view, cx| view.history(false, cx)));
        cx.run_until_parked();
        assert_eq!(fs::read(&output).unwrap(), captured);
        assert_eq!(fs::read(path).unwrap(), original);
        assert_eq!(
            fs::metadata(output).unwrap().permissions().mode() & 0o7777,
            0o700
        );
        cx.update(|_, cx| {
            let view = view.read(cx);
            assert_eq!(view.image.bytes(), original);
            assert!(view.is_dirty());
            assert!(!view.executable_export);
        });
    }
}
