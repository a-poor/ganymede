//! Native application menus. Commands route through the same operations as UI controls.
use crate::Ganymede;
use gpui::{App, Context, KeyBinding, Menu, MenuItem, prelude::*};
gpui::actions!(
    menus,
    [
        NewNotebook,
        NewText,
        NewMarkdown,
        NewPython,
        NewFolder,
        Upload,
        SaveAll,
        SaveAs,
        Rename,
        NextTab,
        PreviousTab,
        ShutdownKernel,
        ShutdownAll,
        ShutdownUnused,
        CutCell,
        CopyCell,
        PasteCell,
        DeleteCell,
        DuplicateCell,
        InsertAbove,
        InsertBelow,
        MoveCellUp,
        MoveCellDown,
        ClearOutput,
        ClearAllOutputs,
        RunCell,
        RunAdvance,
        RunInsert,
        RunAbove,
        RunBelow,
        RunAll,
        RenderMarkdown,
        Interrupt,
        Restart,
        RestartRunAll,
        Reconnect,
        ChangeKernel,
        OpenSettings,
        About,
        Hide,
        HideOthers,
        ShowAll,
        ToggleSidebar,
        ShowFiles,
        ShowKernels,
        Minimize,
        Zoom,
        Fullscreen,
        Documentation,
        ReportIssue
    ]
);

pub fn init(cx: &mut App) {
    cx.on_action(|_: &OpenSettings, cx| crate::open_settings(cx));
    cx.on_action(|_: &About, cx| crate::about::open(cx));
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &Documentation, cx| cx.open_url("https://github.com/a-poor/ganymede#readme"));
    cx.on_action(|_: &ReportIssue, cx| cx.open_url("https://github.com/a-poor/ganymede/issues"));
    cx.on_action(|_: &Minimize, cx| {
        if let Some(w) = cx.active_window() {
            w.update(cx, |_, w, _| w.minimize_window()).ok();
        }
    });
    cx.on_action(|_: &Zoom, cx| {
        if let Some(w) = cx.active_window() {
            w.update(cx, |_, w, _| w.zoom_window()).ok();
        }
    });
    cx.on_action(|_: &Fullscreen, cx| {
        if let Some(w) = cx.active_window() {
            w.update(cx, |_, w, _| w.toggle_fullscreen()).ok();
        }
    });
    let modifier = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };
    cx.bind_keys([
        KeyBinding::new(&format!("{modifier}-,"), OpenSettings, None),
        KeyBinding::new(&format!("{modifier}-h"), Hide, None),
        KeyBinding::new(&format!("{modifier}-alt-h"), HideOthers, None),
        KeyBinding::new(&format!("{modifier}-m"), Minimize, None),
        KeyBinding::new(&format!("{modifier}-b"), ToggleSidebar, Some("Workspace")),
        KeyBinding::new(&format!("{modifier}-shift-s"), SaveAs, Some("Workspace")),
        KeyBinding::new("ctrl-tab", NextTab, Some("Workspace")),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, Some("Workspace")),
        KeyBinding::new(&format!("{modifier}-enter"), RunCell, Some("Notebook")),
        KeyBinding::new("alt-enter", RunInsert, Some("Notebook")),
    ]);
    use MenuItem as I;
    cx.set_menus(vec![
        Menu::new("Ganymede").items([
            I::action("About Ganymede", About),
            I::separator(),
            I::action("Settings…", OpenSettings),
            I::separator(),
            I::os_submenu("Services", gpui::SystemMenuType::Services),
            I::separator(),
            I::action("Hide Ganymede", Hide),
            I::action("Hide Others", HideOthers),
            I::action("Show All", ShowAll),
            I::separator(),
            I::action("Quit Ganymede", crate::Quit),
        ]),
        Menu::new("File").items([
            I::submenu(Menu::new("New").items([
                I::action("Notebook", NewNotebook),
                I::action("Text File", NewText),
                I::action("Markdown File", NewMarkdown),
                I::action("Python File", NewPython),
                I::separator(),
                I::action("Folder…", NewFolder),
            ])),
            I::action("New Launcher", crate::NewTab),
            I::action("New Connection Window", crate::NewWindow),
            I::action("Upload…", Upload),
            I::separator(),
            I::action("Save", crate::Save),
            I::action("Save As…", SaveAs),
            I::action("Save All", SaveAll),
            I::action("Rename…", Rename),
            I::separator(),
            I::action("Close Tab", crate::CloseTab),
        ]),
        Menu::new("Edit").items([
            I::os_action("Undo", crate::code_cell::Undo, gpui::OsAction::Undo),
            I::os_action("Redo", crate::code_cell::Redo, gpui::OsAction::Redo),
            I::separator(),
            I::os_action("Cut", crate::code_cell::Cut, gpui::OsAction::Cut),
            I::os_action("Copy", crate::code_cell::Copy, gpui::OsAction::Copy),
            I::os_action("Paste", crate::code_cell::Paste, gpui::OsAction::Paste),
            I::os_action(
                "Select All",
                crate::code_cell::SelectAll,
                gpui::OsAction::SelectAll,
            ),
            I::separator(),
            I::action("Undo Cell Operation", crate::notebook::UndoStructure),
            I::action("Redo Cell Operation", crate::notebook::RedoStructure),
            I::separator(),
            I::action("Cut Cell", CutCell),
            I::action("Copy Cell", CopyCell),
            I::action("Paste Cell Below", PasteCell),
            I::action("Delete Cell", DeleteCell),
            I::action("Duplicate Cell", DuplicateCell),
            I::action("Insert Cell Above", InsertAbove),
            I::action("Insert Cell Below", InsertBelow),
            I::action("Move Cell Up", MoveCellUp),
            I::action("Move Cell Down", MoveCellDown),
            I::action("Clear Cell Output", ClearOutput),
            I::action("Clear All Outputs", ClearAllOutputs),
            I::separator(),
            I::action("Find…", crate::notebook::FindNotebook),
        ]),
        Menu::new("View").items([
            I::action("Toggle Sidebar", ToggleSidebar),
            I::action("Files", ShowFiles),
            I::action("Kernels", ShowKernels),
            I::separator(),
            I::action("Toggle Full Screen", Fullscreen),
        ]),
        Menu::new("Run").items([
            I::action("Run Cell", RunCell),
            I::action("Run Cell and Advance", RunAdvance),
            I::action("Run Cell and Insert Below", RunInsert),
            I::action("Run All Above", RunAbove),
            I::action("Run Cell and All Below", RunBelow),
            I::action("Run All Cells", RunAll),
            I::action("Render All Markdown Cells", RenderMarkdown),
        ]),
        Menu::new("Kernel").items([
            I::action("Interrupt Kernel", Interrupt),
            I::action("Restart Kernel…", Restart),
            I::action("Restart Kernel and Run All Cells…", RestartRunAll),
            I::action("Reconnect to Kernel", Reconnect),
            I::action("Change Kernel…", ChangeKernel),
            I::separator(),
            I::action("Shut Down Kernel…", ShutdownKernel),
            I::action("Shut Down Unused Kernels…", ShutdownUnused),
            I::action("Shut Down All Kernels…", ShutdownAll),
        ]),
        Menu::new("Window").items([
            I::action("Minimize", Minimize),
            I::action("Zoom", Zoom),
            I::separator(),
            I::action("Next Tab", NextTab),
            I::action("Previous Tab", PreviousTab),
        ]),
        Menu::new("Help").items([
            I::action("Ganymede Documentation", Documentation),
            I::action("Report an Issue…", ReportIssue),
        ]),
    ]);
}

pub fn handlers(
    view: gpui::Stateful<gpui::Div>,
    this: &Ganymede,
    cx: &Context<Ganymede>,
) -> gpui::Stateful<gpui::Div> {
    let workspace = this.workspace.read(cx);
    let has_file = workspace.active_path().is_some();
    let notebook = workspace.active_notebook(cx);
    view.key_context("Workspace")
        .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
            this.set_sidebar_open(!this.sidebar_open, cx)
        }))
        .on_action(cx.listener(|this, _: &ShowFiles, _, cx| {
            this.sidebar_tab = crate::SidebarTab::Files;
            this.set_sidebar_open(true, cx);
        }))
        .on_action(cx.listener(|this, _: &ShowKernels, _, cx| {
            this.sidebar_tab = crate::SidebarTab::Kernels;
            this.workspace
                .update(cx, |w, cx| w.refresh_kernels(false, cx));
            this.set_sidebar_open(true, cx);
        }))
        .on_action(cx.listener(|this, _: &NewNotebook, window, cx| {
            this.workspace.update(cx, |w, cx| {
                w.menu_command(crate::workspace::commands::Command::NewNotebook, window, cx)
            })
        }))
        .on_action(cx.listener(|this, _: &NewText, window, cx| {
            this.workspace.update(cx, |w, cx| {
                w.menu_command(crate::workspace::commands::Command::NewText, window, cx)
            })
        }))
        .on_action(cx.listener(|this, _: &NewMarkdown, window, cx| {
            this.workspace.update(cx, |w, cx| {
                w.menu_command(crate::workspace::commands::Command::NewMarkdown, window, cx)
            })
        }))
        .on_action(cx.listener(|this, _: &NewPython, window, cx| {
            this.workspace.update(cx, |w, cx| {
                w.menu_command(crate::workspace::commands::Command::NewPython, window, cx)
            })
        }))
        .on_action(cx.listener(|this, _: &NewFolder, window, cx| {
            this.workspace.update(cx, |w, cx| {
                w.menu_command(crate::workspace::commands::Command::NewFolder, window, cx)
            })
        }))
        .on_action(cx.listener(|this, _: &Upload, window, cx| {
            this.workspace.update(cx, |w, cx| {
                w.menu_command(crate::workspace::commands::Command::Upload, window, cx)
            })
        }))
        .on_action(cx.listener(|this, _: &SaveAll, window, cx| {
            this.workspace.update(cx, |w, cx| {
                w.menu_command(crate::workspace::commands::Command::SaveAll, window, cx)
            })
        }))
        .when(has_file, |view| {
            view.on_action(cx.listener(|this, _: &SaveAs, window, cx| {
                this.workspace.update(cx, |w, cx| {
                    w.menu_command(crate::workspace::commands::Command::SaveAs, window, cx)
                })
            }))
        })
        .when(has_file, |view| {
            view.on_action(cx.listener(|this, _: &Rename, window, cx| {
                this.workspace.update(cx, |w, cx| {
                    w.menu_command(crate::workspace::commands::Command::Rename, window, cx)
                })
            }))
        })
        .on_action(cx.listener(|this, _: &NextTab, window, cx| {
            this.workspace.update(cx, |w, cx| {
                w.menu_command(crate::workspace::commands::Command::NextTab, window, cx)
            })
        }))
        .on_action(cx.listener(|this, _: &PreviousTab, window, cx| {
            this.workspace.update(cx, |w, cx| {
                w.menu_command(crate::workspace::commands::Command::PreviousTab, window, cx)
            })
        }))
        .when(
            notebook
                .as_ref()
                .is_some_and(|n| n.read(cx).kernel_id().is_some()),
            |view| {
                view.on_action(cx.listener(|this, _: &ShutdownKernel, window, cx| {
                    this.workspace.update(cx, |w, cx| {
                        w.menu_command(
                            crate::workspace::commands::Command::ShutdownKernel,
                            window,
                            cx,
                        )
                    })
                }))
            },
        )
        .on_action(cx.listener(|this, _: &ShutdownAll, window, cx| {
            this.workspace.update(cx, |w, cx| {
                w.menu_command(crate::workspace::commands::Command::ShutdownAll, window, cx)
            })
        }))
        .on_action(cx.listener(|this, _: &ShutdownUnused, window, cx| {
            this.workspace.update(cx, |w, cx| {
                w.menu_command(
                    crate::workspace::commands::Command::ShutdownUnused,
                    window,
                    cx,
                )
            })
        }))
        .when(notebook.is_some(), |view| {
            view.on_action(cx.listener(|this, _: &CutCell, window, cx| {
                let n = this.workspace.read(cx).active_notebook(cx);
                if let Some(n) = n {
                    n.update(cx, |n, cx| {
                        n.menu_command(crate::notebook::commands::Command::CutCell, window, cx)
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &CopyCell, window, cx| {
                let n = this.workspace.read(cx).active_notebook(cx);
                if let Some(n) = n {
                    n.update(cx, |n, cx| {
                        n.menu_command(crate::notebook::commands::Command::CopyCell, window, cx)
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &PasteCell, window, cx| {
                let n = this.workspace.read(cx).active_notebook(cx);
                if let Some(n) = n {
                    n.update(cx, |n, cx| {
                        n.menu_command(crate::notebook::commands::Command::PasteCell, window, cx)
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &DeleteCell, window, cx| {
                let n = this.workspace.read(cx).active_notebook(cx);
                if let Some(n) = n {
                    n.update(cx, |n, cx| {
                        n.menu_command(crate::notebook::commands::Command::DeleteCell, window, cx)
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &DuplicateCell, window, cx| {
                let n = this.workspace.read(cx).active_notebook(cx);
                if let Some(n) = n {
                    n.update(cx, |n, cx| {
                        n.menu_command(
                            crate::notebook::commands::Command::DuplicateCell,
                            window,
                            cx,
                        )
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &InsertAbove, window, cx| {
                let n = this.workspace.read(cx).active_notebook(cx);
                if let Some(n) = n {
                    n.update(cx, |n, cx| {
                        n.menu_command(crate::notebook::commands::Command::InsertAbove, window, cx)
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &InsertBelow, window, cx| {
                let n = this.workspace.read(cx).active_notebook(cx);
                if let Some(n) = n {
                    n.update(cx, |n, cx| {
                        n.menu_command(crate::notebook::commands::Command::InsertBelow, window, cx)
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &MoveCellUp, window, cx| {
                let n = this.workspace.read(cx).active_notebook(cx);
                if let Some(n) = n {
                    n.update(cx, |n, cx| {
                        n.menu_command(crate::notebook::commands::Command::MoveCellUp, window, cx)
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &MoveCellDown, window, cx| {
                let n = this.workspace.read(cx).active_notebook(cx);
                if let Some(n) = n {
                    n.update(cx, |n, cx| {
                        n.menu_command(crate::notebook::commands::Command::MoveCellDown, window, cx)
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &ClearOutput, window, cx| {
                let n = this.workspace.read(cx).active_notebook(cx);
                if let Some(n) = n {
                    n.update(cx, |n, cx| {
                        n.menu_command(crate::notebook::commands::Command::ClearOutput, window, cx)
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &ClearAllOutputs, window, cx| {
                let n = this.workspace.read(cx).active_notebook(cx);
                if let Some(n) = n {
                    n.update(cx, |n, cx| {
                        n.menu_command(
                            crate::notebook::commands::Command::ClearAllOutputs,
                            window,
                            cx,
                        )
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &RunCell, window, cx| {
                let n = this.workspace.read(cx).active_notebook(cx);
                if let Some(n) = n {
                    n.update(cx, |n, cx| {
                        n.menu_command(crate::notebook::commands::Command::RunCell, window, cx)
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &RunAdvance, window, cx| {
                let n = this.workspace.read(cx).active_notebook(cx);
                if let Some(n) = n {
                    n.update(cx, |n, cx| {
                        n.menu_command(crate::notebook::commands::Command::RunAdvance, window, cx)
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &RunInsert, window, cx| {
                let n = this.workspace.read(cx).active_notebook(cx);
                if let Some(n) = n {
                    n.update(cx, |n, cx| {
                        n.menu_command(crate::notebook::commands::Command::RunInsert, window, cx)
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &RunAbove, window, cx| {
                let n = this.workspace.read(cx).active_notebook(cx);
                if let Some(n) = n {
                    n.update(cx, |n, cx| {
                        n.menu_command(crate::notebook::commands::Command::RunAbove, window, cx)
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &RunBelow, window, cx| {
                let n = this.workspace.read(cx).active_notebook(cx);
                if let Some(n) = n {
                    n.update(cx, |n, cx| {
                        n.menu_command(crate::notebook::commands::Command::RunBelow, window, cx)
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &RunAll, window, cx| {
                let n = this.workspace.read(cx).active_notebook(cx);
                if let Some(n) = n {
                    n.update(cx, |n, cx| {
                        n.menu_command(crate::notebook::commands::Command::RunAll, window, cx)
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &RenderMarkdown, window, cx| {
                let n = this.workspace.read(cx).active_notebook(cx);
                if let Some(n) = n {
                    n.update(cx, |n, cx| {
                        n.menu_command(
                            crate::notebook::commands::Command::RenderMarkdown,
                            window,
                            cx,
                        )
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &Interrupt, window, cx| {
                let n = this.workspace.read(cx).active_notebook(cx);
                if let Some(n) = n {
                    n.update(cx, |n, cx| {
                        n.menu_command(crate::notebook::commands::Command::Interrupt, window, cx)
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &Restart, window, cx| {
                let n = this.workspace.read(cx).active_notebook(cx);
                if let Some(n) = n {
                    n.update(cx, |n, cx| {
                        n.menu_command(crate::notebook::commands::Command::Restart, window, cx)
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &RestartRunAll, window, cx| {
                let n = this.workspace.read(cx).active_notebook(cx);
                if let Some(n) = n {
                    n.update(cx, |n, cx| {
                        n.menu_command(
                            crate::notebook::commands::Command::RestartRunAll,
                            window,
                            cx,
                        )
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &Reconnect, window, cx| {
                let n = this.workspace.read(cx).active_notebook(cx);
                if let Some(n) = n {
                    n.update(cx, |n, cx| {
                        n.menu_command(crate::notebook::commands::Command::Reconnect, window, cx)
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &ChangeKernel, window, cx| {
                let n = this.workspace.read(cx).active_notebook(cx);
                if let Some(n) = n {
                    n.update(cx, |n, cx| {
                        n.menu_command(crate::notebook::commands::Command::ChangeKernel, window, cx)
                    });
                }
            }))
        })
}
