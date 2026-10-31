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
            "Compiling {} into a {}-byte placement…",
            site.name, site.capacity
        );
        let job_site = site.clone();
        let work = cx
            .background_executor()
            .spawn(async move { crate::recompiler::compile(&text, &job_site, &cancel) });
        cx.spawn(async move|this,cx|{
   let result=work.await;
   let _=this.update(cx,|this,cx|{
    if this.compilation.generation!=generation||this.revision!=revision{return;}
    this.compilation.running=false;this.compilation.cancel=None;
    if source.read(cx).content_revision()!=source_revision{this.invalidate_compilation();this.message="C source changed; compile again.".into();cx.notify();return;}
    match result{
     Ok(bytes)=>{
      this.offset=site.offset;this.decode();this.lens=Lens::Assembly;
      this.preview=Some((site.offset,bytes,revision));
      this.message=format!("Compiled C preview for {} at {:#x}. Review ABI and instructions, then Apply patch and Export copy. Original file is untouched.",site.name,site.address);
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
