use crate::{
    Target, compose, daemon, documents,
    resources::{Project, Resource},
};
use anyhow::{Context, Result, ensure};
use bollard::{Docker, models::*, query_parameters::*};
use futures_util::StreamExt;
use serde_json::Value;

#[derive(Clone, Copy, Debug)]
pub(crate) enum Action {
    CreateContainer,
    Rename,
    Update,
    Pull,
    Tag,
    Push,
    RemoveImage,
    CreateVolume,
    RemoveVolume,
    CreateNetwork,
    RemoveNetwork,
    Connect,
    Disconnect,
    PruneContainers,
    PruneImages,
    PruneVolumes,
    PruneNetworks,
    PruneBuild,
    Compose(compose::Action),
    ImportProject,
    Source,
    Validate,
    Save,
    Restore,
    Reload,
    Restart,
    ExportImage,
    ImportImage,
    Pause,
    Resume,
    Recreate,
    Start,
    Stop,
    RestartContainer,
    RemoveContainer,
}
impl Action {
    fn sdk(self) -> bool {
        matches!(
            self,
            Self::Start
                | Self::Stop
                | Self::RestartContainer
                | Self::RemoveContainer
                | Self::CreateContainer
                | Self::Rename
                | Self::Update
                | Self::Recreate
                | Self::Pause
                | Self::Resume
                | Self::Pull
                | Self::Tag
                | Self::RemoveImage
                | Self::CreateVolume
                | Self::RemoveVolume
                | Self::CreateNetwork
                | Self::RemoveNetwork
                | Self::Connect
                | Self::Disconnect
                | Self::PruneContainers
                | Self::PruneImages
                | Self::PruneVolumes
                | Self::PruneNetworks
                | Self::PruneBuild
                | Self::ExportImage
                | Self::ImportImage
        )
    }
    pub fn needs_engine(self) -> bool {
        !matches!(
            self,
            Self::Source | Self::Validate | Self::Save | Self::Restore | Self::ImportProject
        )
    }
    pub fn changes_engine(self) -> bool {
        !matches!(
            self,
            Self::Source
                | Self::Validate
                | Self::Save
                | Self::Restore
                | Self::ImportProject
                | Self::ExportImage
                | Self::Compose(compose::Action::Validate | compose::Action::Logs)
        )
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::CreateContainer => "docker.create_container",
            Self::Rename => "docker.rename",
            Self::Update => "docker.update_limits",
            Self::Pull => "docker.pull",
            Self::Tag => "docker.tag",
            Self::Push => "docker.push",
            Self::RemoveImage | Self::RemoveVolume | Self::RemoveNetwork => "docker.remove",
            Self::Pause => "docker.pause_container",
            Self::Resume => "docker.resume_container",
            Self::Recreate => "docker.recreate",
            Self::Start => "docker.start",
            Self::Stop => "docker.stop",
            Self::RestartContainer => "docker.restart",
            Self::RemoveContainer => "docker.remove",
            Self::CreateVolume => "docker.create_volume",
            Self::CreateNetwork => "docker.create_network",
            Self::Connect => "docker.connect",
            Self::Disconnect => "docker.disconnect",
            Self::PruneContainers => "docker.prune_containers",
            Self::PruneImages => "docker.prune_images",
            Self::PruneVolumes => "docker.prune_volumes",
            Self::PruneNetworks => "docker.prune_networks",
            Self::PruneBuild => "docker.prune_build",
            Self::Compose(action) => action.key(),
            Self::ImportProject => "docker.import_project",
            Self::Source => "docker.source_file",
            Self::Validate => "docker.validate",
            Self::Save => "editor.save",
            Self::Restore => "docker.restore_backup",
            Self::Reload => "docker.reload_engine",
            Self::Restart => "docker.restart_engine",
            Self::ExportImage => "docker.export_image",
            Self::ImportImage => "docker.import_image",
        }
    }
    pub fn destructive(self) -> bool {
        matches!(
            self,
            Self::RemoveImage
                | Self::Stop
                | Self::RestartContainer
                | Self::RemoveContainer
                | Self::RemoveVolume
                | Self::RemoveNetwork
                | Self::Disconnect
                | Self::PruneContainers
                | Self::PruneImages
                | Self::PruneVolumes
                | Self::PruneNetworks
                | Self::PruneBuild
                | Self::Compose(compose::Action::Down)
                | Self::Reload
                | Self::Restart
                | Self::Restore
        )
    }
    pub fn fields(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::CreateContainer => &[
                ("docker.name", ""),
                (
                    "docker.json",
                    "{\"Image\":\"alpine:latest\",\"Cmd\":[\"sleep\",\"infinity\"]}",
                ),
            ],
            Self::Rename => &[("docker.name", "")],
            Self::Update => &[(
                "docker.json",
                "{\"Memory\":536870912,\"NanoCpus\":1000000000}",
            )],
            Self::Recreate => &[("docker.json", "{}")],
            Self::Pull | Self::Push => &[("docker.image", "")],
            Self::Tag => &[("docker.repository", ""), ("docker.tag_name", "latest")],
            Self::CreateVolume => &[("docker.json", "{\"Name\":\"\",\"Driver\":\"local\"}")],
            Self::CreateNetwork => &[("docker.json", "{\"Name\":\"\",\"Driver\":\"bridge\"}")],
            Self::Connect | Self::Disconnect => &[("docker.container_id", "")],
            Self::ImportProject => &[("docker.name", ""), ("docker.source_file", "")],
            Self::Source | Self::ExportImage | Self::ImportImage => &[("docker.source_file", "")],
            _ => &[],
        }
    }
}
pub(crate) struct Request {
    pub target: Target,
    pub docker: Option<Docker>,
    pub engine_id: String,
    pub resource: Option<Resource>,
    pub container: Option<String>,
    pub project: Option<Project>,
    pub source: Option<daemon::Source>,
    pub service: Option<String>,
    pub file: Option<documents::File>,
    pub text: String,
    pub values: Vec<String>,
    pub action: Action,
}
pub(crate) struct Outcome {
    pub text: String,
    pub file: Option<documents::File>,
    pub project: Option<Project>,
    pub source: Option<daemon::Source>,
}
pub(crate) async fn execute(request: Request, progress: impl Fn(String)) -> Result<Outcome> {
    if !request.action.sdk()
        || (matches!(request.action, Action::ExportImage | Action::ImportImage)
            && matches!(request.target.endpoint, crate::Endpoint::Ssh(_)))
    {
        return tokio::task::spawn_blocking(move || execute_files(request)).await?;
    }
    let Request {
        docker,
        engine_id,
        resource,
        container,
        values,
        action,
        ..
    } = request;
    let docker = docker.context(crate::t!("docker.loading_details").into_owned())?;
    ensure!(
        docker.info().await?.id.as_deref() == Some(engine_id.as_str()),
        crate::t!("docker.cli_engine_mismatch").into_owned()
    );
    let value = |index| values.get(index).map(String::as_str).unwrap_or_default();
    let json = |index| -> Result<Value> {
        let result: Value = serde_json::from_str(value(index))?;
        ensure!(
            result.is_object(),
            crate::t!("docker.json_object_required").into_owned()
        );
        Ok(result)
    };
    let id = || {
        container
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!(crate::t!("docker.select_container").into_owned()))
    };
    let key = || match &resource {
        Some(Resource::Image(id) | Resource::Network(id) | Resource::Volume(id)) => Ok(id.as_str()),
        _ => anyhow::bail!(crate::t!("docker.select_resource").into_owned()),
    };
    let mut result = Outcome {
        text: String::new(),
        file: None,
        project: None,
        source: None,
    };
    match action {
        Action::ExportImage | Action::ImportImage => {
            result.text = crate::images::archive(
                &docker,
                action,
                match &resource {
                    Some(Resource::Image(id)) => Some(id.as_str()),
                    _ => None,
                },
                value(0),
                progress,
            )
            .await?;
        }
        Action::Start | Action::Stop | Action::RestartContainer | Action::RemoveContainer => {
            let detail = docker.inspect_container(id()?, None).await?;
            let running = detail
                .state
                .as_ref()
                .is_some_and(|state| state.running == Some(true));
            match action {
                Action::Start => {
                    ensure!(!running, crate::t!("docker.state_changed").into_owned());
                    docker.start_container(id()?, None).await?;
                }
                Action::Stop => {
                    ensure!(running, crate::t!("docker.state_changed").into_owned());
                    docker
                        .stop_container(
                            id()?,
                            Some(StopContainerOptionsBuilder::default().t(10).build()),
                        )
                        .await?;
                }
                Action::RestartContainer => {
                    ensure!(running, crate::t!("docker.state_changed").into_owned());
                    docker
                        .restart_container(
                            id()?,
                            Some(RestartContainerOptionsBuilder::default().t(10).build()),
                        )
                        .await?;
                }
                Action::RemoveContainer => {
                    ensure!(!running, crate::t!("docker.remove_running").into_owned());
                    docker
                        .remove_container(
                            id()?,
                            Some(
                                RemoveContainerOptionsBuilder::default()
                                    .force(false)
                                    .v(false)
                                    .build(),
                            ),
                        )
                        .await?;
                }
                _ => unreachable!(),
            }
        }
        Action::CreateContainer => {
            ensure!(
                !value(0).is_empty(),
                crate::t!("docker.name_required").into_owned()
            );
            result.text = serde_json::to_string_pretty(
                &docker
                    .create_container(
                        Some(
                            CreateContainerOptionsBuilder::default()
                                .name(value(0))
                                .build(),
                        ),
                        serde_json::from_value(json(1)?)?,
                    )
                    .await?,
            )?;
        }
        Action::Rename => {
            ensure!(
                !value(0).is_empty(),
                crate::t!("docker.name_required").into_owned()
            );
            docker
                .rename_container(
                    id()?,
                    RenameContainerOptionsBuilder::default()
                        .name(value(0))
                        .build(),
                )
                .await?;
        }
        Action::Pause => docker.pause_container(id()?).await?,
        Action::Resume => docker.unpause_container(id()?).await?,
        Action::Recreate => {
            let detail = docker.inspect_container(id()?, None).await?;
            ensure!(
                !detail
                    .state
                    .as_ref()
                    .is_some_and(|state| state.running == Some(true)),
                crate::t!("docker.stop_before_recreate").into_owned()
            );
            let name = detail
                .name
                .as_deref()
                .unwrap_or_default()
                .trim_start_matches('/');
            ensure!(
                !name.is_empty(),
                crate::t!("docker.name_required").into_owned()
            );
            let config: ContainerCreateBody = serde_json::from_value(json(0)?)?;
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos();
            let backup = format!("{name}-tshell-{nonce}");
            docker
                .rename_container(
                    id()?,
                    RenameContainerOptionsBuilder::default()
                        .name(&backup)
                        .build(),
                )
                .await?;
            let created = docker
                .create_container(
                    Some(CreateContainerOptionsBuilder::default().name(name).build()),
                    config,
                )
                .await;
            match created {
                Ok(created) => {
                    result.text =
                        crate::t!("docker.recreated", id = created.id, backup = backup).into_owned()
                }
                Err(error) => {
                    let restored = docker
                        .rename_container(
                            id()?,
                            RenameContainerOptionsBuilder::default().name(name).build(),
                        )
                        .await;
                    return Err(anyhow::anyhow!(
                        "{error}\n{}: {}",
                        backup,
                        restored
                            .err()
                            .map(|error| error.to_string())
                            .unwrap_or_else(|| crate::t!("docker.name_restored").into_owned())
                    ));
                }
            }
        }
        Action::Update => {
            result.text = serde_json::to_string_pretty(
                &docker
                    .update_container(id()?, serde_json::from_value(json(0)?)?)
                    .await?,
            )?;
        }
        Action::Pull => {
            ensure!(
                !value(0).is_empty(),
                crate::t!("docker.name_required").into_owned()
            );
            let mut stream = docker.create_image(
                Some(
                    CreateImageOptionsBuilder::default()
                        .from_image(value(0))
                        .build(),
                ),
                None,
                None,
            );
            while let Some(entry) = stream.next().await {
                let entry = entry?;
                progress(format!(
                    "{} {}",
                    entry.status.unwrap_or_default(),
                    serde_json::to_string(&entry.progress_detail).unwrap_or_default()
                ));
            }
        }
        Action::Tag => {
            ensure!(
                !value(0).is_empty() && !value(1).is_empty(),
                crate::t!("docker.name_required").into_owned()
            );
            docker
                .tag_image(
                    key()?,
                    Some(
                        TagImageOptionsBuilder::default()
                            .repo(value(0))
                            .tag(value(1))
                            .build(),
                    ),
                )
                .await?;
        }
        Action::RemoveImage => {
            result.text = serde_json::to_string_pretty(
                &docker
                    .remove_image(
                        key()?,
                        Some(RemoveImageOptionsBuilder::default().force(false).build()),
                        None,
                    )
                    .await?,
            )?;
        }
        Action::CreateVolume => {
            let config: VolumeCreateRequest = serde_json::from_value(json(0)?)?;
            ensure!(
                config.name.as_ref().is_some_and(|name| !name.is_empty()),
                crate::t!("docker.name_required").into_owned()
            );
            result.text = serde_json::to_string_pretty(&docker.create_volume(config).await?)?;
        }
        Action::RemoveVolume => {
            docker
                .remove_volume(
                    key()?,
                    Some(RemoveVolumeOptionsBuilder::default().force(false).build()),
                )
                .await?
        }
        Action::CreateNetwork => {
            let config: NetworkCreateRequest = serde_json::from_value(json(0)?)?;
            ensure!(
                !config.name.is_empty(),
                crate::t!("docker.name_required").into_owned()
            );
            result.text = serde_json::to_string_pretty(&docker.create_network(config).await?)?;
        }
        Action::RemoveNetwork => docker.remove_network(key()?).await?,
        Action::Connect => {
            docker
                .connect_network(
                    key()?,
                    NetworkConnectRequest {
                        container: value(0).into(),
                        ..Default::default()
                    },
                )
                .await?
        }
        Action::Disconnect => {
            docker
                .disconnect_network(
                    key()?,
                    NetworkDisconnectRequest {
                        container: value(0).into(),
                        force: Some(false),
                    },
                )
                .await?
        }
        Action::PruneContainers => {
            result.text = serde_json::to_string_pretty(
                &docker
                    .prune_containers(None::<PruneContainersOptions>)
                    .await?,
            )?
        }
        Action::PruneImages => {
            result.text = serde_json::to_string_pretty(
                &docker.prune_images(None::<PruneImagesOptions>).await?,
            )?
        }
        Action::PruneVolumes => {
            result.text = serde_json::to_string_pretty(
                &docker.prune_volumes(None::<PruneVolumesOptions>).await?,
            )?
        }
        Action::PruneNetworks => {
            result.text = serde_json::to_string_pretty(
                &docker.prune_networks(None::<PruneNetworksOptions>).await?,
            )?
        }
        Action::PruneBuild => {
            result.text =
                serde_json::to_string_pretty(&docker.prune_build(None::<PruneBuildOptions>).await?)?
        }
        _ => anyhow::bail!(crate::t!("docker.invalid_command").into_owned()),
    }
    if result.text.is_empty() {
        result.text = crate::t!("docker.operation_complete").into_owned();
    }
    Ok(result)
}
fn execute_files(request: Request) -> Result<Outcome> {
    let Request {
        target,
        engine_id,
        resource,
        project,
        source,
        service,
        file,
        text,
        values,
        action,
        ..
    } = request;
    if resource == Some(Resource::Daemon)
        && !matches!(action, Action::Source)
        && !engine_id.is_empty()
    {
        if let Some(bound) = source.as_ref().and_then(|source| source.engine_id.as_ref()) {
            ensure!(
                *bound == engine_id,
                crate::t!("docker.config_source_required").into_owned()
            );
        }
    }
    let mut result = Outcome {
        text: String::new(),
        file: None,
        project: None,
        source: None,
    };
    let input = |index| values.get(index).map(String::as_str).unwrap_or_default();
    let project = || {
        project
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!(crate::t!("docker.compose_source_missing").into_owned()))
    };
    let file = || {
        file.as_ref()
            .ok_or_else(|| anyhow::anyhow!(crate::t!("docker.config_source_required").into_owned()))
    };
    let source = || {
        source
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!(crate::t!("docker.config_source_required").into_owned()))
    };
    match action {
        Action::Compose(action) => {
            result.text = compose::execute_service(
                &target,
                &engine_id,
                project()?,
                action,
                service.as_deref(),
            )?
        }
        Action::ImportProject => {
            ensure!(
                !input(0).is_empty(),
                crate::t!("docker.name_required").into_owned()
            );
            let file = documents::read(&target, input(1).into())?;
            ensure!(
                file.original.is_some(),
                crate::t!("docker.compose_source_missing").into_owned()
            );
            let project = Project {
                name: input(0).into(),
                files: vec![file.path.clone()],
                directory: documents::parent(&file.path, &target),
            };
            compose::execute(&target, &engine_id, &project, compose::Action::Validate)?;
            result.project = Some(project);
            result.file = Some(file);
        }
        Action::Source => {
            result.file = Some(documents::read(&target, input(0).into())?);
            if resource == Some(Resource::Daemon) {
                result.source = Some(
                    if let Some(source) = source().ok().filter(|source| {
                        source.path == input(0)
                            && (engine_id.is_empty()
                                || source.engine_id.as_deref() == Some(engine_id.as_str()))
                    }) {
                        source.clone()
                    } else {
                        daemon::Source {
                            path: input(0).into(),
                            manager: daemon::Manager::Manual,
                            engine_id: None,
                        }
                    },
                );
            } else {
                let mut changed = project()?.clone();
                if !changed.files.iter().any(|path| path == input(0)) {
                    if changed.files.is_empty() {
                        changed.files.push(input(0).into());
                    } else {
                        changed.files[0] = input(0).into();
                    }
                }
                if changed.directory.is_none() {
                    changed.directory = documents::parent(input(0), &target);
                }
                result.project = Some(changed);
            }
        }
        Action::Validate | Action::Save => {
            result.text = if resource == Some(Resource::Daemon) {
                daemon::validate(&target, source()?, &text)?
            } else {
                compose::validate_document(&target, &engine_id, project()?, file()?, &text)?
            };
            if matches!(action, Action::Save) {
                result.file = Some(documents::save(&target, file()?, &text)?);
            }
        }
        Action::Restore => {
            let backup = file()?
                .backup
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!(crate::t!("docker.no_backup").into_owned()))?;
            let text = documents::read(&target, backup.clone())?
                .original
                .ok_or_else(|| anyhow::anyhow!(crate::t!("docker.no_backup").into_owned()))?;
            result.file = Some(documents::save(&target, file()?, &text)?);
        }
        Action::Reload | Action::Restart => {
            ensure!(
                file()?.original.as_deref() == Some(text.as_str()),
                crate::t!("docker.save_first").into_owned()
            );
            ensure!(
                documents::read(&target, file()?.path.clone())?.original == file()?.original,
                crate::t!("docker.file_changed").into_owned()
            );
            daemon::validate(&target, source()?, &text)?;
            result.text = daemon::apply(
                &target,
                &engine_id,
                source()?,
                matches!(action, Action::Restart),
            )?;
        }
        Action::Push => {
            let binding = crate::cli::Binding::resolve(&target, &engine_id)?;
            ensure!(
                !input(0).is_empty(),
                crate::t!("docker.name_required").into_owned()
            );
            if let Some(Resource::Image(id)) = &resource {
                let bytes = binding.host.run(binding.request([
                    "image".into(),
                    "inspect".into(),
                    "--format".into(),
                    "{{.Id}}".into(),
                    input(0).into(),
                ]))?;
                ensure!(
                    String::from_utf8_lossy(&bytes).trim() == id,
                    crate::t!("docker.state_changed").into_owned()
                );
            }
            result.text = binding.run(binding.request(["push".into(), input(0).into()]))?;
        }
        Action::ExportImage | Action::ImportImage => {
            let binding = crate::cli::Binding::resolve(&target, &engine_id)?;
            ensure!(
                !input(0).is_empty(),
                crate::t!("docker.name_required").into_owned()
            );
            let arguments = if matches!(action, Action::ExportImage) {
                let Some(Resource::Image(id)) = &resource else {
                    anyhow::bail!(crate::t!("docker.select_resource").into_owned())
                };
                documents::ensure_missing(&target, input(0))?;
                vec![
                    "image".into(),
                    "save".into(),
                    "--output".into(),
                    input(0).into(),
                    id.clone(),
                ]
            } else {
                vec![
                    "image".into(),
                    "load".into(),
                    "--input".into(),
                    input(0).into(),
                ]
            };
            result.text = binding.run(binding.request(arguments))?;
        }
        _ => anyhow::bail!(crate::t!("docker.invalid_command").into_owned()),
    }
    if result.text.is_empty() {
        result.text = crate::t!("docker.operation_complete").into_owned();
    }
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn daemon_save_keeps_unknown_keys_and_a_backup_when_the_engine_is_unavailable() {
        let path = std::env::temp_dir().join(format!(
            "tshell-offline-daemon-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, "{\"debug\":false}").unwrap();
        let target = Target::local("fixture".into(), None);
        let file = documents::read(&target, path.to_string_lossy().into()).unwrap();
        let request = Request {
            target,
            docker: None,
            engine_id: String::new(),
            resource: Some(Resource::Daemon),
            container: None,
            project: None,
            source: Some(daemon::Source {
                path: path.to_string_lossy().into(),
                manager: daemon::Manager::Desktop,
                engine_id: None,
            }),
            service: None,
            file: Some(file),
            text: "{\"debug\":true,\"future-setting\":{\"enabled\":true}}".into(),
            values: Vec::new(),
            action: Action::Save,
        };
        let outcome = execute_files(request).unwrap();
        let file = outcome.file.unwrap();
        let value: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["future-setting"]["enabled"], true);
        assert_eq!(
            std::fs::read_to_string(file.backup.as_ref().unwrap()).unwrap(),
            "{\"debug\":false}"
        );
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(file.backup.unwrap()).unwrap();
    }
    #[test]
    fn daemon_restart_rejects_a_file_changed_after_saving() {
        let path = std::env::temp_dir().join(format!(
            "tshell-changed-daemon-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let original = "{\"debug\":false}";
        let external = "{\"debug\":true}";
        std::fs::write(&path, original).unwrap();
        let target = Target::local("fixture".into(), None);
        let file = documents::read(&target, path.to_string_lossy().into()).unwrap();
        std::fs::write(&path, external).unwrap();
        let result = execute_files(Request {
            target,
            docker: None,
            engine_id: String::new(),
            resource: Some(Resource::Daemon),
            container: None,
            project: None,
            source: None,
            service: None,
            file: Some(file),
            text: original.into(),
            values: Vec::new(),
            action: Action::Restart,
        });
        assert_eq!(
            result.err().unwrap().to_string(),
            crate::t!("docker.file_changed")
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), external);
        std::fs::remove_file(path).unwrap();
    }
}
