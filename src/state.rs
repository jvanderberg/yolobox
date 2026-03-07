use crate::ports::{DEFAULT_GUEST_PORTS, PortMapping, build_port_mappings, choose_port_block};
use std::collections::hash_map::DefaultHasher;
use std::env;
use std::ffi::CString;
use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
#[cfg(target_os = "macos")]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn clonefile(
        src: *const std::ffi::c_char,
        dst: *const std::ffi::c_char,
        flags: std::ffi::c_int,
    ) -> std::ffi::c_int;
}

const DEFAULT_ROOTFS_MIB: u64 = 32 * 1024;

#[derive(Clone, Debug)]
pub struct BaseImage {
    pub id: String,
    pub name: String,
    pub image_path: PathBuf,
    pub source_path: PathBuf,
    pub size_bytes: u64,
    pub created_unix: u64,
}

impl BaseImage {
    pub fn summary_lines(&self) -> Vec<String> {
        vec![
            format!("base: {}", self.name),
            format!("id: {}", self.id),
            format!("image: {}", self.image_path.display()),
            format!("source: {}", self.source_path.display()),
            format!("size: {} MiB", bytes_to_mib(self.size_bytes)),
        ]
    }
}

#[derive(Clone, Debug)]
pub struct InstancePaths {
    pub instance_dir: PathBuf,
    pub checkout_dir: PathBuf,
    pub rootfs_path: PathBuf,
    pub metadata_path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct Instance {
    pub id: String,
    pub repo: String,
    pub branch: String,
    pub instance_dir: PathBuf,
    pub base_image_id: String,
    pub base_image_name: String,
    pub base_image_path: PathBuf,
    pub checkout_dir: PathBuf,
    pub rootfs_path: PathBuf,
    pub rootfs_mb: u64,
    pub host_port_base: u16,
    pub ports: Vec<PortMapping>,
    pub created_unix: u64,
}

impl Instance {
    pub fn summary_lines(&self) -> Vec<String> {
        let ports = self
            .ports
            .iter()
            .map(|mapping| format!("{}->{}", mapping.host, mapping.guest))
            .collect::<Vec<_>>()
            .join(", ");

        vec![
            format!("instance: {}", self.id),
            format!("repo: {}", self.repo),
            format!("branch: {}", self.branch),
            format!("base: {} ({})", self.base_image_name, self.base_image_id),
            format!("instance_dir: {}", self.instance_dir.display()),
            format!("base_image: {}", self.base_image_path.display()),
            format!("checkout: {}", self.checkout_dir.display()),
            format!(
                "rootfs: {} ({} MiB)",
                self.rootfs_path.display(),
                self.rootfs_mb
            ),
            format!("ports: {ports}"),
        ]
    }
}

pub fn app_home() -> Result<PathBuf, String> {
    if let Ok(path) = env::var("VIBEBOX_HOME") {
        return Ok(PathBuf::from(path));
    }

    let home = env::var("HOME").map_err(|_| "HOME is not set".to_string())?;
    Ok(PathBuf::from(home)
        .join(".local")
        .join("state")
        .join("vibebox"))
}

pub fn import_base_image(name: &str, source: &Path) -> Result<BaseImage, String> {
    if !source.is_file() {
        return Err(format!(
            "base image source {} is not a file",
            source.display()
        ));
    }

    let canonical_source = fs::canonicalize(source).unwrap_or_else(|_| source.to_path_buf());
    let paths = base_paths(name)?;
    if paths.metadata_path.exists() || paths.image_path.exists() {
        return Err(format!("base image {name} already exists"));
    }

    fs::create_dir_all(&paths.base_dir).map_err(io_err)?;
    clone_or_copy_file(&canonical_source, &paths.image_path)?;
    set_readonly(&paths.image_path, true)?;

    let size_bytes = fs::metadata(&paths.image_path).map_err(io_err)?.len();
    let base = BaseImage {
        id: paths.id,
        name: name.to_string(),
        image_path: paths.image_path,
        source_path: canonical_source,
        size_bytes,
        created_unix: now_unix(),
    };

    save_base_image(&paths.metadata_path, &base)?;
    Ok(base)
}

pub fn find_base_image(name: &str) -> Result<Option<BaseImage>, String> {
    let paths = base_paths(name)?;
    load_base_image(&paths.metadata_path)
}

pub fn list_base_images() -> Result<Vec<BaseImage>, String> {
    let home = app_home()?;
    let bases_root = home.join("base-images");
    if !bases_root.exists() {
        return Ok(Vec::new());
    }

    let mut images = Vec::new();
    for entry in fs::read_dir(bases_root).map_err(io_err)? {
        let entry = entry.map_err(io_err)?;
        let metadata_path = entry.path().join("base.env");
        if let Some(base) = load_base_image(&metadata_path)? {
            images.push(base);
        }
    }

    images.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(images)
}

pub fn ensure_instance(
    repo: &str,
    branch: &str,
    requested_base: Option<&str>,
) -> Result<Instance, String> {
    let paths = paths_for(repo, branch)?;
    fs::create_dir_all(&paths.checkout_dir).map_err(io_err)?;
    fs::create_dir_all(paths.instance_dir.join("vm")).map_err(io_err)?;

    let existing = load_instance(&paths.metadata_path)?;
    let base = resolve_base_image(existing.as_ref(), requested_base)?;

    let current_id = paths
        .instance_dir
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let used_host_bases = list_instances()?
        .into_iter()
        .filter(|instance| instance.id != current_id)
        .map(|instance| instance.host_port_base)
        .collect::<Vec<_>>();
    let host_port_base = choose_port_block(
        &format!("{repo}|{branch}"),
        existing.as_ref().map(|instance| instance.host_port_base),
        &used_host_bases,
    )?;

    let target_rootfs_mib = desired_rootfs_mib(bytes_to_mib(base.size_bytes));
    ensure_branch_rootfs(&base.image_path, &paths.rootfs_path, target_rootfs_mib)?;
    let rootfs_mb = bytes_to_mib(fs::metadata(&paths.rootfs_path).map_err(io_err)?.len());

    let instance = Instance {
        id: current_id,
        repo: repo.to_string(),
        branch: branch.to_string(),
        instance_dir: paths.instance_dir,
        base_image_id: base.id.clone(),
        base_image_name: base.name.clone(),
        base_image_path: base.image_path.clone(),
        checkout_dir: paths.checkout_dir,
        rootfs_path: paths.rootfs_path,
        rootfs_mb,
        host_port_base,
        ports: build_port_mappings(host_port_base, &DEFAULT_GUEST_PORTS),
        created_unix: existing
            .map(|instance| instance.created_unix)
            .unwrap_or_else(now_unix),
    };

    save_instance(&paths.metadata_path, &instance)?;
    Ok(instance)
}

pub fn find_instance(repo: &str, branch: &str) -> Result<Option<Instance>, String> {
    let paths = paths_for(repo, branch)?;
    load_instance(&paths.metadata_path)
}

pub fn list_instances() -> Result<Vec<Instance>, String> {
    let home = app_home()?;
    let instances_root = home.join("instances");
    if !instances_root.exists() {
        return Ok(Vec::new());
    }

    let mut instances = Vec::new();
    for entry in fs::read_dir(instances_root).map_err(io_err)? {
        let entry = entry.map_err(io_err)?;
        let metadata_path = entry.path().join("instance.env");
        if let Some(instance) = load_instance(&metadata_path)? {
            instances.push(instance);
        }
    }

    instances.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(instances)
}

pub fn destroy_instance(repo: &str, branch: &str) -> Result<Option<PathBuf>, String> {
    let paths = paths_for(repo, branch)?;
    if !paths.instance_dir.exists() {
        return Ok(None);
    }

    fs::remove_dir_all(&paths.instance_dir).map_err(io_err)?;
    Ok(Some(paths.instance_dir))
}

fn resolve_base_image(
    existing: Option<&Instance>,
    requested_base: Option<&str>,
) -> Result<BaseImage, String> {
    match (existing, requested_base) {
        (Some(instance), Some(name)) => {
            let requested = find_base_image(name)?
                .ok_or_else(|| format!("base image {name} does not exist; import it first"))?;
            if requested.id != instance.base_image_id {
                return Err(format!(
                    "instance is pinned to base {} but {} was requested",
                    instance.base_image_name, name
                ));
            }
            Ok(requested)
        }
        (Some(instance), None) => find_base_image(&instance.base_image_name)?
            .filter(|base| base.id == instance.base_image_id)
            .ok_or_else(|| {
                format!(
                    "base image {} for this instance is missing; re-import it or destroy the instance",
                    instance.base_image_name
                )
            }),
        (None, Some(name)) => find_base_image(name)?
            .ok_or_else(|| format!("base image {name} does not exist; import it first")),
        (None, None) => Err("launch requires --base for a new branch instance".to_string()),
    }
}

fn ensure_branch_rootfs(
    base_image_path: &Path,
    rootfs_path: &Path,
    target_rootfs_mib: u64,
) -> Result<(), String> {
    let parent = rootfs_path
        .parent()
        .ok_or_else(|| "invalid rootfs path".to_string())?;
    fs::create_dir_all(parent).map_err(io_err)?;

    if !rootfs_path.exists() {
        clone_or_copy_file(base_image_path, rootfs_path)?;
    }

    set_readonly(rootfs_path, false)?;
    expand_rootfs_if_needed(rootfs_path, target_rootfs_mib)?;
    Ok(())
}

fn expand_rootfs_if_needed(rootfs_path: &Path, target_rootfs_mib: u64) -> Result<(), String> {
    let current_size = fs::metadata(rootfs_path).map_err(io_err)?.len();
    let target_size = mib_to_bytes(target_rootfs_mib);
    if current_size >= target_size {
        return Ok(());
    }

    OpenOptions::new()
        .write(true)
        .open(rootfs_path)
        .map_err(io_err)?
        .set_len(target_size)
        .map_err(io_err)?;
    Ok(())
}

fn clone_or_copy_file(source: &Path, destination: &Path) -> Result<(), String> {
    if destination.exists() {
        return Err(format!(
            "destination {} already exists",
            destination.display()
        ));
    }

    #[cfg(target_os = "macos")]
    {
        let src = CString::new(source.as_os_str().as_bytes()).map_err(|err| err.to_string())?;
        let dst =
            CString::new(destination.as_os_str().as_bytes()).map_err(|err| err.to_string())?;
        let cloned = unsafe { clonefile(src.as_ptr(), dst.as_ptr(), 0) };
        if cloned == 0 {
            return Ok(());
        }
    }

    fs::copy(source, destination).map_err(io_err)?;
    Ok(())
}

fn set_readonly(path: &Path, readonly: bool) -> Result<(), String> {
    let mut permissions = fs::metadata(path).map_err(io_err)?.permissions();
    #[cfg(unix)]
    {
        permissions.set_mode(if readonly { 0o444 } else { 0o644 });
    }
    #[cfg(not(unix))]
    permissions.set_readonly(readonly);
    fs::set_permissions(path, permissions).map_err(io_err)?;
    Ok(())
}

fn bytes_to_mib(size_bytes: u64) -> u64 {
    const MIB: u64 = 1024 * 1024;
    size_bytes.div_ceil(MIB)
}

fn mib_to_bytes(size_mib: u64) -> u64 {
    size_mib.saturating_mul(1024 * 1024)
}

fn desired_rootfs_mib(base_image_mib: u64) -> u64 {
    env::var("VIBEBOX_ROOTFS_MIB")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_ROOTFS_MIB)
        .max(base_image_mib)
}

fn base_paths(name: &str) -> Result<BasePaths, String> {
    let home = app_home()?;
    let id = base_image_id(name)?;
    let base_dir = home.join("base-images").join(&id);
    Ok(BasePaths {
        id,
        metadata_path: base_dir.join("base.env"),
        image_path: base_dir.join("base.img"),
        base_dir,
    })
}

fn paths_for(repo: &str, branch: &str) -> Result<InstancePaths, String> {
    let home = app_home()?;
    let id = instance_id(repo, branch);
    let instance_dir = home.join("instances").join(&id);
    Ok(InstancePaths {
        checkout_dir: instance_dir.join("checkout"),
        rootfs_path: instance_dir.join("vm").join("branch.img"),
        metadata_path: instance_dir.join("instance.env"),
        instance_dir,
    })
}

fn base_image_id(name: &str) -> Result<String, String> {
    let id = slugify(name);
    if id.is_empty() {
        return Err("base image name must contain at least one alphanumeric character".to_string());
    }
    Ok(id)
}

fn instance_id(repo: &str, branch: &str) -> String {
    let repo_slug = slugify(repo);
    let branch_slug = slugify(branch);
    let mut hasher = DefaultHasher::new();
    repo.hash(&mut hasher);
    branch.hash(&mut hasher);
    let digest = hasher.finish();
    format!("{repo_slug}--{branch_slug}--{digest:08x}")
}

fn slugify(value: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;

    for ch in value.chars() {
        let normalized = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else {
            '-'
        };
        if normalized == '-' {
            if !previous_dash {
                slug.push(normalized);
                previous_dash = true;
            }
        } else {
            slug.push(normalized);
            previous_dash = false;
        }
    }

    slug.trim_matches('-').chars().take(48).collect()
}

fn load_instance(path: &Path) -> Result<Option<Instance>, String> {
    if !path.exists() {
        return Ok(None);
    }

    let values = load_env_file(path)?;
    Ok(Some(Instance {
        id: required_string(&values, "id", path)?,
        repo: required_string(&values, "repo", path)?,
        branch: required_string(&values, "branch", path)?,
        instance_dir: path
            .parent()
            .ok_or_else(|| format!("{} has no parent directory", path.display()))?
            .to_path_buf(),
        base_image_id: required_string(&values, "base_image_id", path)?,
        base_image_name: required_string(&values, "base_image_name", path)?,
        base_image_path: PathBuf::from(required_string(&values, "base_image_path", path)?),
        checkout_dir: PathBuf::from(required_string(&values, "checkout_dir", path)?),
        rootfs_path: PathBuf::from(required_string(&values, "rootfs_path", path)?),
        rootfs_mb: required_u64(&values, "rootfs_mb", path)?,
        host_port_base: required_u16(&values, "host_port_base", path)?,
        ports: parse_ports(&required_string(&values, "ports", path)?)?,
        created_unix: required_u64(&values, "created_unix", path)?,
    }))
}

fn save_instance(path: &Path, instance: &Instance) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "invalid metadata path".to_string())?;
    fs::create_dir_all(parent).map_err(io_err)?;

    let mut file = File::create(path).map_err(io_err)?;
    writeln!(file, "id={}", instance.id).map_err(io_err)?;
    writeln!(file, "repo={}", instance.repo).map_err(io_err)?;
    writeln!(file, "branch={}", instance.branch).map_err(io_err)?;
    writeln!(file, "base_image_id={}", instance.base_image_id).map_err(io_err)?;
    writeln!(file, "base_image_name={}", instance.base_image_name).map_err(io_err)?;
    writeln!(
        file,
        "base_image_path={}",
        instance.base_image_path.display()
    )
    .map_err(io_err)?;
    writeln!(file, "checkout_dir={}", instance.checkout_dir.display()).map_err(io_err)?;
    writeln!(file, "rootfs_path={}", instance.rootfs_path.display()).map_err(io_err)?;
    writeln!(file, "rootfs_mb={}", instance.rootfs_mb).map_err(io_err)?;
    writeln!(file, "host_port_base={}", instance.host_port_base).map_err(io_err)?;
    writeln!(file, "ports={}", encode_ports(&instance.ports)).map_err(io_err)?;
    writeln!(file, "created_unix={}", instance.created_unix).map_err(io_err)?;
    Ok(())
}

fn load_base_image(path: &Path) -> Result<Option<BaseImage>, String> {
    if !path.exists() {
        return Ok(None);
    }

    let values = load_env_file(path)?;
    Ok(Some(BaseImage {
        id: required_string(&values, "id", path)?,
        name: required_string(&values, "name", path)?,
        image_path: PathBuf::from(required_string(&values, "image_path", path)?),
        source_path: PathBuf::from(required_string(&values, "source_path", path)?),
        size_bytes: required_u64(&values, "size_bytes", path)?,
        created_unix: required_u64(&values, "created_unix", path)?,
    }))
}

fn save_base_image(path: &Path, image: &BaseImage) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "invalid base metadata path".to_string())?;
    fs::create_dir_all(parent).map_err(io_err)?;

    let mut file = File::create(path).map_err(io_err)?;
    writeln!(file, "id={}", image.id).map_err(io_err)?;
    writeln!(file, "name={}", image.name).map_err(io_err)?;
    writeln!(file, "image_path={}", image.image_path.display()).map_err(io_err)?;
    writeln!(file, "source_path={}", image.source_path.display()).map_err(io_err)?;
    writeln!(file, "size_bytes={}", image.size_bytes).map_err(io_err)?;
    writeln!(file, "created_unix={}", image.created_unix).map_err(io_err)?;
    Ok(())
}

fn load_env_file(path: &Path) -> Result<Vec<(String, String)>, String> {
    let mut content = String::new();
    File::open(path)
        .map_err(io_err)?
        .read_to_string(&mut content)
        .map_err(io_err)?;

    Ok(content
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect())
}

fn required_string(values: &[(String, String)], key: &str, path: &Path) -> Result<String, String> {
    values
        .iter()
        .find(|(candidate, _)| candidate == key)
        .map(|(_, value)| value.clone())
        .ok_or_else(|| format!("{} is missing {}", path.display(), key))
}

fn required_u64(values: &[(String, String)], key: &str, path: &Path) -> Result<u64, String> {
    required_string(values, key, path)?
        .parse::<u64>()
        .map_err(|_| format!("{} has invalid {}", path.display(), key))
}

fn required_u16(values: &[(String, String)], key: &str, path: &Path) -> Result<u16, String> {
    required_string(values, key, path)?
        .parse::<u16>()
        .map_err(|_| format!("{} has invalid {}", path.display(), key))
}

fn encode_ports(ports: &[PortMapping]) -> String {
    ports
        .iter()
        .map(|mapping| format!("{}:{}", mapping.host, mapping.guest))
        .collect::<Vec<_>>()
        .join(",")
}

fn parse_ports(value: &str) -> Result<Vec<PortMapping>, String> {
    value
        .split(',')
        .filter(|item| !item.is_empty())
        .map(|item| {
            let (host, guest) = item
                .split_once(':')
                .ok_or_else(|| format!("invalid port mapping: {item}"))?;
            Ok(PortMapping {
                host: host
                    .parse::<u16>()
                    .map_err(|_| format!("invalid host port: {host}"))?,
                guest: guest
                    .parse::<u16>()
                    .map_err(|_| format!("invalid guest port: {guest}"))?,
            })
        })
        .collect()
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn io_err(err: impl std::fmt::Display) -> String {
    err.to_string()
}

struct BasePaths {
    id: String,
    base_dir: PathBuf,
    metadata_path: PathBuf,
    image_path: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::desired_rootfs_mib;
    use std::env;

    #[test]
    fn desired_rootfs_defaults_to_large_sparse_disk() {
        unsafe {
            env::remove_var("VIBEBOX_ROOTFS_MIB");
        }
        assert_eq!(desired_rootfs_mib(2252), 32 * 1024);
    }

    #[test]
    fn desired_rootfs_respects_larger_env_override() {
        unsafe {
            env::set_var("VIBEBOX_ROOTFS_MIB", "65536");
        }
        assert_eq!(desired_rootfs_mib(2252), 65536);
        unsafe {
            env::remove_var("VIBEBOX_ROOTFS_MIB");
        }
    }

    #[test]
    fn desired_rootfs_never_shrinks_below_base_image() {
        unsafe {
            env::set_var("VIBEBOX_ROOTFS_MIB", "1024");
        }
        assert_eq!(desired_rootfs_mib(4096), 4096);
        unsafe {
            env::remove_var("VIBEBOX_ROOTFS_MIB");
        }
    }
}
