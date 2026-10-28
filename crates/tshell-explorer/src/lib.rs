extern crate gpui_kit as gpui;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    Icon, Selectable, Sizable, WindowExt,
    breadcrumb::{Breadcrumb, BreadcrumbItem},
    button::{Button, ButtonVariants},
    input::{EditorState, Input, InputEvent, InputState},
    list::ListItem,
    menu::{ContextMenuExt, PopupMenuItem},
    notification::Notification,
    select::{SearchableVec, SelectEvent, SelectState},
    tree::{Tree, TreeEvent, TreeState},
};
use gpui_kit::{prelude::*, *};
use std::{cell::RefCell, collections::BTreeSet, path::PathBuf, rc::Rc};
use tshell_host::{SshConfig, files as file_ops, sftp as remote_files};
use tshell_ui::appearance::Palette;
use tshell_ui::controls::{self as style, button, icon_button, workspace_dialog};
use tshell_ui::editor as editor_theme;
pub use tshell_ui::{i18n, t};
use workbench::NodeView;
mod editor_search;
pub mod explorer;
mod file_access;
pub mod preview;
mod view;
pub mod workbench;
pub use explorer::FileClipboard;
#[derive(Clone)]
pub enum Event {
    Activated,
    Saved(PathBuf),
    OpenTerminal(PathBuf),
    Upload {
        remote: remote_files::Session,
        paths: Vec<PathBuf>,
        target: PathBuf,
    },
    Download {
        remote: remote_files::Session,
        source: PathBuf,
        destination: PathBuf,
    },
    RequestOperation(file_ops::Operation),
    FilesChanged(file_ops::Operation),
}
#[derive(Default)]
pub struct SessionFileState {
    pub path: Option<PathBuf>,
    pub text: String,
    pub image: Option<std::sync::Arc<Image>>,
    pub pdf: Option<std::sync::Arc<Vec<u8>>>,
    pub preview: Option<preview::Document>,
    pub preview_mode: bool,
    pub dirty: bool,
    pub language: String,
    pub saved_text: String,
}

pub struct Explorer {
    pub host: Option<SshConfig>,
    remote: Option<remote_files::Session>,
    pub visible: bool,
    pub document_local: bool,
    pub palette: Palette,
    pub terminal_palette: Palette,
    pub font_family: String,
    pub font_size: f32,
    pub message: Option<String>,
    pub need_focus: bool,
    pub default_cwd: PathBuf,
    pub cwd: PathBuf,
    pub file_tree: Entity<TreeState>,
    pub file_clipboard: Rc<RefCell<Option<FileClipboard>>>,
    pub file_operation: bool,
    pub file_picker: Entity<SelectState<SearchableVec<String>>>,
    pub file_editor: Entity<EditorState>,
    pub pending_file_state: Option<SessionFileState>,
    pub file_link_position: Option<(PathBuf, (u32, u32))>,
    pub open_file: Option<PathBuf>,
    pub image_preview: Option<std::sync::Arc<Image>>,
    pub pdf_preview: Option<std::sync::Arc<Vec<u8>>>,
    pub file_preview: Option<preview::Document>,
    #[cfg(windows)]
    pub web_preview: Option<preview::WebPreview>,
    #[cfg(windows)]
    pub web_preview_failed: bool,
    #[cfg(windows)]
    web_preview_initializing: bool,
    #[cfg(windows)]
    web_preview_generation: u64,
    pub preview_mode: bool,
    pub editor_dirty: bool,
    pub editor_language: String,
    pub saved_file_text: String,
    pub editor_search: Entity<InputState>,
    pub editor_replace: Entity<InputState>,
    pub editor_search_revision: u64,
    pub file_request: u64,
    pub tree_request: u64,
    pub tree_loading: BTreeSet<String>,
    pub tree_loaded: BTreeSet<String>,
    pub file_loading: Option<String>,
    pub file_saving: bool,
    pub pending_file_paths: Option<Vec<String>>,
}
impl EventEmitter<Event> for Explorer {}
impl Explorer {
    pub fn new(
        host: Option<SshConfig>,
        cwd: PathBuf,
        palette: Palette,
        terminal_palette: Palette,
        font_family: String,
        font_size: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let tree_items = Vec::new();
        let file_paths = Vec::new();
        let file_tree = cx.new(|cx| TreeState::new(cx).items(tree_items));
        let file_picker = cx.new(|cx| {
            SelectState::new(SearchableVec::new(file_paths), None, window, cx).searchable(true)
        });
        let file_editor = cx.new(|cx| {
            let mut editor = EditorState::new(window, cx).language("plaintext");
            editor.set_highlighter_factory(editor_theme::factory(), cx);
            editor
        });

        let mut this = Self {
            remote: host.clone().map(remote_files::Session::new),
            host,
            visible: true,
            document_local: false,
            palette,
            terminal_palette,
            font_family,
            font_size,
            message: None,
            need_focus: false,
            default_cwd: cwd.clone(),
            cwd,
            file_tree,
            file_clipboard: Rc::new(RefCell::new(None)),
            file_operation: false,
            file_picker,
            file_editor,
            pending_file_state: None,
            file_link_position: None,
            open_file: None,
            image_preview: None,
            pdf_preview: None,
            file_preview: None,
            #[cfg(windows)]
            web_preview: None,
            #[cfg(windows)]
            web_preview_failed: false,
            #[cfg(windows)]
            web_preview_initializing: false,
            #[cfg(windows)]
            web_preview_generation: 0,
            preview_mode: false,
            editor_dirty: false,
            editor_language: "plaintext".into(),
            saved_file_text: String::new(),
            editor_search: cx.new(|cx| {
                InputState::new(window, cx).placeholder(crate::t!("ws.placeholder_search"))
            }),
            editor_replace: cx.new(|cx| {
                InputState::new(window, cx).placeholder(crate::t!("ws.placeholder_replace"))
            }),
            editor_search_revision: 0,
            file_request: 0,
            tree_request: 0,
            tree_loading: BTreeSet::new(),
            tree_loaded: BTreeSet::new(),
            file_loading: None,
            file_saving: false,
            pending_file_paths: None,
        };
        cx.subscribe_in(&this.file_editor, window, |this, _, event, _, cx| {
            if matches!(event, InputEvent::Change) && this.open_file.is_some() {
                this.editor_dirty = true;
                if this.preview_mode {
                    let text = this.file_editor.read(cx).value().to_string();
                    this.preview_mode = preview::can_preview(&this.editor_language, &text);
                    this.file_preview = this
                        .preview_mode
                        .then(|| preview::parse(&this.editor_language, &text));
                }
                cx.notify();
            }
        })
        .detach();
        cx.subscribe_in(&this.file_picker, window, |this, _, event, window, cx| {
            if let SelectEvent::Confirm(Some(path)) = event {
                let path = if this.host.is_some() {
                    PathBuf::from(path)
                } else {
                    this.cwd.join(path)
                };
                this.open_path(path, window, cx);
            }
        })
        .detach();
        cx.subscribe(&this.file_tree, |this, _, event, cx| {
            if let TreeEvent::Expanded(id) = event {
                this.expand_directory(id.to_string(), cx);
            }
        })
        .detach();
        this.subscribe_editor_search(window, cx);

        this
    }
    pub fn queue_download(
        &mut self,
        remote: remote_files::Session,
        source: PathBuf,
        destination: PathBuf,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.emit(Event::Download {
            remote,
            source,
            destination,
        });
    }
    pub fn queue_upload(
        &mut self,
        remote: remote_files::Session,
        paths: Vec<PathBuf>,
        target: PathBuf,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.emit(Event::Upload {
            remote,
            paths,
            target,
        });
    }
}

impl Explorer {
    pub fn busy(&self) -> bool {
        self.file_saving
            || self.file_operation
            || self.editor_dirty
            || self
                .pending_file_state
                .as_ref()
                .is_some_and(|document| document.dirty)
    }
    pub fn operation_conflicts(&self, operation: &file_ops::Operation) -> bool {
        let path = match operation {
            file_ops::Operation::Rename { from, .. } | file_ops::Operation::Delete(from) => from,
            _ => return false,
        };
        self.file_saving
            || ((self.editor_dirty
                && self.open_file.as_ref().is_some_and(|open| {
                    tshell_host::HostPath::for_host(
                        open,
                        self.host.is_some() && !self.document_local,
                    )
                    .is_within(&tshell_host::HostPath::for_host(path, self.host.is_some()))
                }))
                || self.pending_file_state.as_ref().is_some_and(|document| {
                    document.dirty
                        && document.path.as_ref().is_some_and(|open| {
                            tshell_host::HostPath::for_host(
                                open,
                                self.host.is_some() && !self.document_local,
                            )
                            .is_within(&tshell_host::HostPath::for_host(path, self.host.is_some()))
                        })
                }))
    }
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.visible = visible;
        #[cfg(windows)]
        if !visible {
            if let Some(web) = &mut self.web_preview {
                web.hide();
            }
        }
        cx.notify();
    }
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open_file.is_some() && self.image_preview.is_none() && !self.preview_mode {
            self.file_editor
                .update(cx, |editor, cx| editor.focus(window, cx));
        }
        self.need_focus = false;
    }
}
