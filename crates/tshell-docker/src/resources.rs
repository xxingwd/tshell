use crate::{Target, panel::container_name};
use anyhow::Result;
use bollard::{
    Docker,
    models::{ContainerSummary, ImageSummary, Network, SystemDataUsageResponse, Volume},
    query_parameters::{ListImagesOptionsBuilder, ListVolumesOptions},
};
use gpui_kit::component::input::{EditorState, InputState};
use gpui_kit::{prelude::*, *};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResourcePage {
    Containers,
    Compose,
    Images,
    More,
}
impl ResourcePage {
    pub const ALL: [Self; 4] = [Self::Containers, Self::Compose, Self::Images, Self::More];
    pub fn key(self) -> &'static str {
        match self {
            Self::Containers => "docker.containers",
            Self::Compose => "docker.compose",
            Self::Images => "docker.images",
            Self::More => "docker.more",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ImageReference {
    pub id: String,
    pub tag: Option<String>,
}
impl From<String> for ImageReference {
    fn from(id: String) -> Self {
        Self { id, tag: None }
    }
}
pub(crate) fn image_references(images: &[ImageSummary]) -> Vec<ImageReference> {
    let mut references = Vec::new();
    for image in images {
        let tags: Vec<_> = image
            .repo_tags
            .iter()
            .filter(|tag| !tag.is_empty() && tag.as_str() != "<none>:<none>")
            .collect();
        if tags.is_empty() {
            references.push(ImageReference::from(image.id.clone()));
        } else {
            references.extend(tags.into_iter().map(|tag| ImageReference {
                id: image.id.clone(),
                tag: Some(tag.clone()),
            }));
        }
    }
    references.sort_by(|left, right| {
        left.tag
            .is_none()
            .cmp(&right.tag.is_none())
            .then_with(|| left.tag.cmp(&right.tag))
            .then_with(|| left.id.cmp(&right.id))
    });
    references.dedup();
    references
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Resource {
    Container(String),
    Project(String),
    Image(ImageReference),
    Volume(String),
    Network(String),
    Info,
    Disk,
    Events,
    Daemon,
}
impl Resource {
    pub fn inspect_key(&self) -> Self {
        match self {
            Self::Image(image) => Self::Image(image.id.clone().into()),
            _ => self.clone(),
        }
    }
    pub fn page(&self) -> ResourcePage {
        match self {
            Self::Container(_) => ResourcePage::Containers,
            Self::Project(_) => ResourcePage::Compose,
            Self::Image(_) => ResourcePage::Images,
            _ => ResourcePage::More,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Project {
    pub name: String,
    pub files: Vec<String>,
    pub directory: Option<String>,
}
pub(crate) fn projects(containers: &[ContainerSummary]) -> Vec<Project> {
    let mut projects = BTreeMap::new();
    for container in containers {
        let Some(labels) = &container.labels else {
            continue;
        };
        let Some(name) = labels
            .get("com.docker.compose.project")
            .filter(|name| !name.is_empty())
        else {
            continue;
        };
        let project = projects.entry(name.clone()).or_insert_with(|| Project {
            name: name.clone(),
            files: Vec::new(),
            directory: None,
        });
        if let Some(files) = labels.get("com.docker.compose.project.config_files") {
            for file in files.split(',').filter(|file| !file.is_empty()) {
                if !project.files.iter().any(|old| old == file) {
                    project.files.push(file.to_owned());
                }
            }
        }
        if project.directory.is_none() {
            project.directory = labels
                .get("com.docker.compose.project.working_dir")
                .cloned();
        }
    }
    projects.into_values().collect()
}
pub(crate) fn service_name(container: &ContainerSummary) -> String {
    container
        .labels
        .as_ref()
        .and_then(|labels| labels.get("com.docker.compose.service"))
        .cloned()
        .unwrap_or_else(|| container_name(container))
}
pub(crate) fn image_name(image: &ImageSummary) -> String {
    let tags: Vec<_> = image
        .repo_tags
        .iter()
        .filter(|tag| tag.as_str() != "<none>:<none>")
        .cloned()
        .collect();
    if tags.is_empty() {
        short_id(&image.id).to_owned()
    } else {
        tags.into_iter().min().unwrap_or_default()
    }
}
pub(crate) fn short_id(id: &str) -> &str {
    let id = id.strip_prefix("sha256:").unwrap_or(id);
    id.get(..12).unwrap_or(id)
}
pub(crate) fn format_bytes(bytes: u64) -> String {
    let (mut size, mut unit) = (bytes as f64, 0);
    while size >= 1024. && unit < 4 {
        size /= 1024.;
        unit += 1;
    }
    format!("{size:.1} {}", ["B", "KiB", "MiB", "GiB", "TiB"][unit])
}
pub(crate) fn format_image_size(bytes: u64) -> String {
    let (mut size, mut unit) = (bytes as f64, 0);
    while size >= 1000. && unit < 4 {
        size /= 1000.;
        unit += 1;
    }
    format!("{size:.1} {}", ["B", "kB", "MB", "GB", "TB"][unit])
}
pub(crate) struct Snapshot {
    pub images: Option<Vec<ImageSummary>>,
    pub volumes: Option<Vec<Volume>>,
    pub networks: Option<Vec<Network>>,
    pub disk: Option<SystemDataUsageResponse>,
}
pub(crate) async fn list(docker: &Docker, page: ResourcePage) -> Result<Snapshot> {
    let mut result = Snapshot {
        images: None,
        volumes: None,
        networks: None,
        disk: None,
    };
    match page {
        ResourcePage::Images => {
            result.images = Some(
                docker
                    .list_images(Some(ListImagesOptionsBuilder::default().all(false).build()))
                    .await?,
            );
        }
        ResourcePage::More => {
            let (volumes, networks, disk) = tokio::try_join!(
                docker.list_volumes(None::<ListVolumesOptions>),
                docker.list_networks(None),
                docker.df(None)
            )?;
            result.volumes = Some(volumes.volumes.unwrap_or_default());
            result.networks = Some(networks);
            result.disk = Some(disk);
        }
        ResourcePage::Containers | ResourcePage::Compose => {}
    }
    Ok(result)
}
pub(crate) async fn inspect(docker: &Docker, resource: &Resource) -> Result<Value> {
    Ok(match resource {
        Resource::Image(image) => {
            let (detail, history) = tokio::try_join!(
                docker.inspect_image(&image.id),
                docker.image_history(&image.id)
            )?;
            let mut value = serde_json::to_value(detail)?;
            value["History"] = serde_json::to_value(history)?;
            value
        }
        Resource::Volume(name) => serde_json::to_value(docker.inspect_volume(name).await?)?,
        Resource::Network(id) => serde_json::to_value(docker.inspect_network(id, None).await?)?,
        Resource::Info => serde_json::to_value(docker.info().await?)?,
        Resource::Disk => serde_json::to_value(docker.df(None).await?)?,
        Resource::Events => Value::Array(Vec::new()),
        Resource::Container(id) => serde_json::to_value(docker.inspect_container(id, None).await?)?,
        Resource::Project(_) | Resource::Daemon => Value::Null,
    })
}
pub(crate) struct Resources {
    pub collection: Option<crate::browser::Collection>,
    pub inspector: bool,
    pub table_focus: FocusHandle,
    pub table_scrolls: BTreeMap<crate::browser::Collection, UniformListScrollHandle>,
    pub page: ResourcePage,
    pub selected: BTreeMap<ResourcePage, Resource>,
    pub queries: BTreeMap<ResourcePage, Entity<InputState>>,
    pub focus: FocusHandle,
    pub images: Vec<ImageSummary>,
    pub volumes: Vec<Volume>,
    pub networks: Vec<Network>,
    pub disk: Option<SystemDataUsageResponse>,
    pub projects: Vec<Project>,
    pub imported: Vec<Project>,
    pub preview: BTreeMap<Resource, Value>,
    pub raw: BTreeSet<ResourcePage>,
    pub editor: Entity<EditorState>,
    pub output: Entity<EditorState>,
    pub output_text: String,
    pub output_visible: bool,
    pub output_resource: Option<Resource>,
    pub events: Vec<Value>,
    pub documents: BTreeMap<String, crate::documents::Document>,
    pub active_files: BTreeMap<Resource, String>,
    pub services: BTreeMap<String, String>,
    pub daemon: Option<crate::daemon::Source>,
}
impl Resources {
    pub fn new(window: &mut Window, cx: &mut Context<crate::DockerView>) -> Self {
        let queries = ResourcePage::ALL
            .into_iter()
            .map(|page| {
                let query = cx.new(|cx| {
                    InputState::new(window, cx).placeholder(crate::t!("docker.search_resources"))
                });
                cx.observe(&query, |_, _, cx| cx.notify()).detach();
                (page, query)
            })
            .collect();
        let editor = cx.new(|cx| {
            let mut editor = EditorState::new(window, cx).language("json");
            editor.set_highlighter_factory(tshell_ui::editor::factory(), cx);
            editor.set_readonly(true, cx);
            editor
        });
        let output = cx.new(|cx| {
            let mut editor = EditorState::new(window, cx).language("plaintext");
            editor.set_readonly(true, cx);
            editor
        });
        Self {
            collection: Some(crate::browser::Collection::Containers),
            inspector: false,
            table_focus: cx.focus_handle(),
            table_scrolls: crate::browser::Collection::ALL
                .into_iter()
                .map(|collection| (collection, UniformListScrollHandle::new()))
                .collect(),
            focus: cx.focus_handle(),
            page: ResourcePage::Containers,
            selected: BTreeMap::new(),
            queries,
            images: Vec::new(),
            volumes: Vec::new(),
            networks: Vec::new(),
            disk: None,
            projects: Vec::new(),
            imported: Vec::new(),
            preview: BTreeMap::new(),
            raw: BTreeSet::new(),
            editor,
            output,
            output_text: String::new(),
            output_visible: false,
            output_resource: None,
            events: Vec::new(),
            documents: BTreeMap::new(),
            active_files: BTreeMap::new(),
            services: BTreeMap::new(),
            daemon: None,
        }
    }
    pub fn selection(&self) -> Option<&Resource> {
        self.selected.get(&self.page)
    }
    pub fn raw(&self) -> bool {
        self.raw.contains(&self.page)
    }
    pub fn document(&self, resource: &Resource) -> Option<&crate::documents::Document> {
        self.active_files
            .get(resource)
            .and_then(|path| self.documents.get(path))
    }
    pub fn apply(&mut self, snapshot: Snapshot) {
        if let Some(images) = snapshot.images {
            self.images = images;
        }
        if let Some(volumes) = snapshot.volumes {
            self.volumes = volumes;
        }
        if let Some(networks) = snapshot.networks {
            self.networks = networks;
        }
        if let Some(disk) = snapshot.disk {
            self.disk = Some(disk);
        }
    }
    pub fn update_projects(&mut self, containers: &[ContainerSummary]) {
        self.projects = projects(containers);
        for project in &self.imported {
            if let Some(existing) = self
                .projects
                .iter_mut()
                .find(|entry| entry.name == project.name)
            {
                existing.files = project.files.clone();
                existing.directory = project.directory.clone();
            } else {
                self.projects.push(project.clone());
            }
        }
        self.projects.sort_by(|a, b| a.name.cmp(&b.name));
    }
    pub fn name(&self, resource: &Resource) -> String {
        match resource {
            Resource::Container(id) => short_id(id).into(),
            Resource::Project(name) | Resource::Volume(name) => name.clone(),
            Resource::Image(reference) => reference.tag.clone().unwrap_or_else(|| {
                self.images
                    .iter()
                    .find(|image| image.id == reference.id)
                    .map(image_name)
                    .unwrap_or_else(|| short_id(&reference.id).into())
            }),
            Resource::Network(id) => self
                .networks
                .iter()
                .find(|network| network.id.as_ref() == Some(id))
                .and_then(|network| network.name.clone())
                .unwrap_or_else(|| short_id(id).into()),
            Resource::Info => crate::t!("docker.engine_info").into_owned(),
            Resource::Disk => crate::t!("docker.storage").into_owned(),
            Resource::Events => crate::t!("docker.events").into_owned(),
            Resource::Daemon => crate::t!("docker.engine_config").into_owned(),
        }
    }
}
pub(crate) fn host(target: &Target) -> tshell_host::Host {
    match &target.endpoint {
        crate::Endpoint::Local { .. } => tshell_host::Host::Local,
        crate::Endpoint::Ssh(config) => tshell_host::Host::Ssh(config.clone()),
    }
}
#[cfg(test)]
mod tests {
    use super::{ContainerSummary, ImageSummary, image_references, projects};
    #[test]
    fn image_aliases_remain_separate_and_untagged_images_follow_named_references() {
        let images = [
            ImageSummary {
                id: "sha256:000000000000-untagged".into(),
                repo_tags: vec!["<none>:<none>".into()],
                ..Default::default()
            },
            ImageSummary {
                id: "sha256:shared-redis".into(),
                repo_tags: vec![
                    "redis:7.4-alpine".into(),
                    "redis:7-alpine".into(),
                    "redis:7-alpine".into(),
                ],
                ..Default::default()
            },
            ImageSummary {
                id: "sha256:caddy".into(),
                repo_tags: vec!["caddy:latest".into()],
                ..Default::default()
            },
            ImageSummary {
                id: "sha256:111111111111-untagged".into(),
                ..Default::default()
            },
        ];
        let references = image_references(&images);
        assert_eq!(
            references
                .iter()
                .map(|image| image.tag.as_deref())
                .collect::<Vec<_>>(),
            [
                Some("caddy:latest"),
                Some("redis:7-alpine"),
                Some("redis:7.4-alpine"),
                None,
                None
            ]
        );
        assert_eq!(references[1].id, references[2].id);
        assert_ne!(references[1], references[2]);
        assert_eq!(references[3].id, images[0].id);
        assert_eq!(references[4].id, images[3].id);
    }
    #[test]
    fn compose_projects_keep_exact_identity_and_all_source_files() {
        let labels = std::collections::HashMap::from([
            ("com.docker.compose.project".into(), "app.prod".into()),
            (
                "com.docker.compose.project.config_files".into(),
                "/srv/base.yml,/srv/override.yml".into(),
            ),
        ]);
        let containers = vec![
            ContainerSummary {
                labels: Some(labels.clone()),
                ..Default::default()
            },
            ContainerSummary {
                labels: Some(labels),
                ..Default::default()
            },
            ContainerSummary::default(),
        ];
        let result = projects(&containers);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "app.prod");
        assert_eq!(result[0].files, ["/srv/base.yml", "/srv/override.yml"]);
    }
}
