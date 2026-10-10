use crate::{Target, cli::Binding, resources::Project};
use anyhow::{Result, ensure};
use tshell_host::{HostPath, PosixPath};

#[derive(Clone, Copy, Debug)]
pub(crate) enum Action {
    Up,
    Start,
    Stop,
    Restart,
    Pull,
    Build,
    Down,
    Validate,
    Logs,
}
impl Action {
    pub fn key(self) -> &'static str {
        match self {
            Self::Up => "docker.compose_up",
            Self::Start => "docker.compose_start",
            Self::Stop => "docker.compose_stop",
            Self::Restart => "docker.compose_restart",
            Self::Pull => "docker.pull",
            Self::Build => "docker.build",
            Self::Down => "docker.compose_down",
            Self::Validate => "docker.validate",
            Self::Logs => "docker.logs",
        }
    }
}
pub(crate) fn execute(
    target: &Target,
    engine_id: &str,
    project: &Project,
    action: Action,
) -> Result<String> {
    execute_service(target, engine_id, project, action, None)
}
pub(crate) fn execute_service(
    target: &Target,
    engine_id: &str,
    project: &Project,
    action: Action,
    service: Option<&str>,
) -> Result<String> {
    ensure!(
        !project.name.is_empty() && !project.files.is_empty(),
        crate::t!("docker.compose_source_missing").into_owned()
    );
    let binding = if matches!(action, Action::Validate) {
        Binding {
            host: crate::resources::host(target),
            arguments: crate::cli::arguments(target)?,
            engine_id: String::new(),
        }
    } else {
        Binding::resolve(target, engine_id)?
    };
    let mut arguments = vec![
        "compose".into(),
        "--project-name".into(),
        project.name.clone(),
    ];
    for file in &project.files {
        arguments.extend(["--file".into(), file.clone()]);
    }
    if let Some(directory) = &project.directory {
        arguments.extend(["--project-directory".into(), directory.clone()]);
    }
    arguments.extend(match action {
        Action::Up => vec!["up".into(), "--detach".into()],
        Action::Start => vec!["start".into()],
        Action::Stop => vec!["stop".into()],
        Action::Restart => vec!["restart".into()],
        Action::Pull => vec!["pull".into()],
        Action::Build => vec!["build".into()],
        Action::Down => vec!["down".into()],
        Action::Validate => vec!["config".into(), "--quiet".into()],
        Action::Logs => vec![
            "logs".into(),
            "--no-color".into(),
            "--tail".into(),
            "1000".into(),
        ],
    });
    if !matches!(action, Action::Down | Action::Validate) {
        if let Some(service) = service {
            arguments.push(service.into());
        }
    }
    let mut request = binding.request(arguments);
    if let Some(directory) = &project.directory {
        request.directory = Some(match &target.endpoint {
            crate::Endpoint::Local { .. } => HostPath::Local(directory.into()),
            crate::Endpoint::Ssh(_) => HostPath::Remote(PosixPath::new(directory)),
        });
    }
    if matches!(action, Action::Validate) {
        Ok(crate::cli::output_text(binding.host.run_output(request)?))
    } else {
        binding.run(request)
    }
}
pub(crate) fn validate_document(
    target: &Target,
    engine_id: &str,
    project: &Project,
    file: &crate::documents::File,
    text: &str,
) -> Result<String> {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let temporary = format!("{}.tshell-{nonce}.yaml", file.path);
    let result = (|| {
        crate::documents::ensure_missing(target, &temporary)?;
        crate::documents::save(
            target,
            &crate::documents::File {
                path: temporary.clone(),
                original: None,
                backup: None,
            },
            text,
        )?;
        let mut candidate = project.clone();
        let index = candidate
            .files
            .iter()
            .position(|path| path == &file.path)
            .ok_or_else(|| {
                anyhow::anyhow!(crate::t!("docker.compose_source_missing").into_owned())
            })?;
        candidate.files[index] = temporary.clone();
        if candidate.directory.is_none() {
            candidate.directory = crate::documents::parent(&file.path, target);
        }
        execute(target, engine_id, &candidate, Action::Validate)
    })();
    crate::documents::remove_temporary(target, &temporary);
    result
}
pub(crate) fn discover(target: &Target, engine_id: &str) -> Result<Vec<Project>> {
    let binding = Binding::resolve(target, engine_id)?;
    let bytes = binding.host.run(binding.request([
        "compose".into(),
        "ls".into(),
        "--all".into(),
        "--format".into(),
        "json".into(),
    ]))?;
    let entries: Vec<serde_json::Value> = serde_json::from_slice(&bytes)?;
    Ok(entries
        .into_iter()
        .filter_map(|entry| {
            let name = entry.get("Name")?.as_str()?.to_owned();
            let files = entry
                .get("ConfigFiles")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .split(',')
                .filter(|path| !path.is_empty())
                .map(str::to_owned)
                .collect();
            Some(Project {
                name,
                files,
                directory: None,
            })
        })
        .collect())
}
