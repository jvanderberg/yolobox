use crate::network::VmnetConfig;
use crate::ports::PortMapping;
use crate::state::Instance;
use std::env;
use std::fs;
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_CPUS: u8 = 4;
const DEFAULT_MEMORY_MIB: u32 = 8192;
const DEFAULT_WORKSPACE_TAG: &str = "workspace";
const DEFAULT_SSH_READY_TIMEOUT_SECS: u64 = 300;

pub enum LaunchMode {
    External(PathBuf),
    Krunkit,
    Shell,
}

pub struct RuntimePlan {
    pub mode: LaunchMode,
}

pub struct LaunchConfig {
    pub require_vm: bool,
    pub cpus: u8,
    pub memory_mib: u32,
    pub cloud_init_image: Option<PathBuf>,
    pub cloud_init_user: Option<String>,
    pub hostname: Option<String>,
    pub ssh_pubkey_path: Option<PathBuf>,
    pub ssh_private_key_path: Option<PathBuf>,
    pub init_script_path: Option<PathBuf>,
    pub vmnet: Option<VmnetConfig>,
}

pub fn default_cpus() -> u8 {
    DEFAULT_CPUS
}

pub fn default_memory_mib() -> u32 {
    DEFAULT_MEMORY_MIB
}

pub fn command_exists(name: &str) -> bool {
    env::var_os("PATH")
        .map(|paths| env::split_paths(&paths).any(|dir| dir.join(name).exists()))
        .unwrap_or(false)
}

pub fn stop_instance_vm(instance_dir: &Path) -> Result<(), String> {
    for pid in instance_process_pids(instance_dir)? {
        terminate_pid(pid)?;
    }
    stop_stale_vm(&instance_dir.join("runtime").join("krunkit.pid"))
}

pub fn resolve_runtime(force_shell: bool) -> RuntimePlan {
    if force_shell {
        return RuntimePlan {
            mode: LaunchMode::Shell,
        };
    }

    if let Some(launcher) = env::var_os("VIBEBOX_VM_LAUNCHER").map(PathBuf::from) {
        return RuntimePlan {
            mode: LaunchMode::External(launcher),
        };
    }

    if command_exists("krunkit") {
        return RuntimePlan {
            mode: LaunchMode::Krunkit,
        };
    }

    RuntimePlan {
        mode: LaunchMode::Shell,
    }
}

pub fn launch(
    plan: RuntimePlan,
    instance: &Instance,
    config: &LaunchConfig,
) -> Result<i32, String> {
    match plan.mode {
        LaunchMode::Shell => {
            if config.require_vm {
                return Err(
                    "vm runtime requested, but neither VIBEBOX_VM_LAUNCHER nor krunkit is available"
                        .to_string(),
                );
            }
            launch_shell(instance)
        }
        LaunchMode::External(launcher) => launch_external(&launcher, instance, config),
        LaunchMode::Krunkit => launch_krunkit(instance, config),
    }
}

pub fn launch_summary(instance: &Instance, config: &LaunchConfig) -> Vec<String> {
    let mut lines = instance.summary_lines();
    lines.push(format!("vm_cpus: {}", config.cpus));
    lines.push(format!("vm_memory_mib: {}", config.memory_mib));
    if let Some(path) = &config.cloud_init_image {
        lines.push(format!("cloud_init: {}", path.display()));
    }
    if let Some(user) = &config.cloud_init_user {
        lines.push(format!("guest_user: {}", user));
    }
    if let Some(hostname) = &config.hostname {
        lines.push(format!("guest_hostname: {}", hostname));
    }
    if let Some(path) = &config.ssh_pubkey_path {
        lines.push(format!("ssh_pubkey: {}", path.display()));
    }
    if let Some(path) = &config.ssh_private_key_path {
        lines.push(format!("ssh_private_key: {}", path.display()));
    }
    if let Some(path) = &config.init_script_path {
        lines.push(format!("init_script: {}", path.display()));
    }
    if let Some(vmnet) = &config.vmnet {
        lines.extend(vmnet.summary_lines());
    }

    if let Some(launcher) = env::var_os("VIBEBOX_VM_LAUNCHER") {
        lines.push(format!(
            "vm_launcher: external {}",
            PathBuf::from(launcher).display()
        ));
    } else if command_exists("krunkit") {
        if config.vmnet.is_some() {
            lines.push("vm_launcher: builtin krunkit via vmnet-client".to_string());
            lines.push("network: SSH + local port forwards".to_string());
        } else {
            lines.push("vm_launcher: builtin krunkit without host networking".to_string());
        }
    } else {
        lines.push(if config.require_vm {
            "vm_launcher: unavailable".to_string()
        } else {
            "vm_launcher: local shell".to_string()
        });
    }

    if config.cloud_init_image.is_some() {
        lines.push("guest_workspace: auto-mounted at /workspace via cloud-init".to_string());
    } else {
        lines.push(format!(
            "guest_workspace: mount -t virtiofs {} /workspace",
            DEFAULT_WORKSPACE_TAG
        ));
    }
    lines
}

fn launch_shell(instance: &Instance) -> Result<i32, String> {
    let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let status = Command::new(shell)
        .current_dir(&instance.checkout_dir)
        .env("VIBEBOX_INSTANCE", &instance.id)
        .env("VIBEBOX_CHECKOUT", &instance.checkout_dir)
        .env("VIBEBOX_BASE_IMAGE", &instance.base_image_path)
        .env("VIBEBOX_BASE_IMAGE_ID", &instance.base_image_id)
        .env("VIBEBOX_ROOTFS", &instance.rootfs_path)
        .env("VIBEBOX_BRANCH", &instance.branch)
        .env("VIBEBOX_PORTS", encode_env_ports(&instance.ports))
        .status()
        .map_err(|err| err.to_string())?;
    Ok(status.code().unwrap_or_default())
}

fn launch_external(
    launcher: &Path,
    instance: &Instance,
    config: &LaunchConfig,
) -> Result<i32, String> {
    let mut command = Command::new(launcher);
    command
        .current_dir(&instance.checkout_dir)
        .env("VIBEBOX_INSTANCE", &instance.id)
        .env("VIBEBOX_REPO", &instance.repo)
        .env("VIBEBOX_BRANCH", &instance.branch)
        .env("VIBEBOX_CHECKOUT", &instance.checkout_dir)
        .env("VIBEBOX_BASE_IMAGE", &instance.base_image_path)
        .env("VIBEBOX_BASE_IMAGE_ID", &instance.base_image_id)
        .env("VIBEBOX_ROOTFS", &instance.rootfs_path)
        .env("VIBEBOX_ROOTFS_MB", instance.rootfs_mb.to_string())
        .env("VIBEBOX_CPUS", config.cpus.to_string())
        .env("VIBEBOX_MEMORY_MIB", config.memory_mib.to_string())
        .env(
            "VIBEBOX_CLOUD_INIT_IMAGE",
            config
                .cloud_init_image
                .as_ref()
                .map(|path| path.as_os_str())
                .unwrap_or_default(),
        )
        .env(
            "VIBEBOX_CLOUD_INIT_USER",
            config.cloud_init_user.as_deref().unwrap_or_default(),
        )
        .env(
            "VIBEBOX_HOSTNAME",
            config.hostname.as_deref().unwrap_or_default(),
        )
        .env("VIBEBOX_PORTS", encode_env_ports(&instance.ports));

    if let Some(vmnet) = &config.vmnet {
        command
            .env("VIBEBOX_GUEST_IP", &vmnet.guest_ip)
            .env("VIBEBOX_GUEST_GATEWAY", &vmnet.gateway_ip)
            .env("VIBEBOX_GUEST_MAC", &vmnet.mac_address)
            .env("VIBEBOX_INTERFACE_ID", &vmnet.interface_id);
    }

    if let Some(path) = &config.ssh_private_key_path {
        command.env("VIBEBOX_SSH_PRIVATE_KEY", path);
    }

    let status = command.status().map_err(|err| err.to_string())?;
    Ok(status.code().unwrap_or_default())
}

fn launch_krunkit(instance: &Instance, config: &LaunchConfig) -> Result<i32, String> {
    let vmnet = config
        .vmnet
        .as_ref()
        .ok_or_else(|| "built-in VM networking requires vmnet-client".to_string())?;
    let ssh_user = config
        .cloud_init_user
        .as_deref()
        .ok_or_else(|| "SSH guest user is required for built-in VM launch".to_string())?;
    let ssh_private_key = config
        .ssh_private_key_path
        .as_ref()
        .ok_or_else(|| "SSH private key is required for built-in VM launch".to_string())?;

    let runtime_dir = instance.instance_dir.join("runtime");
    fs::create_dir_all(&runtime_dir).map_err(|err| err.to_string())?;

    let console_log = runtime_dir.join("console.log");
    let krunkit_log = runtime_dir.join("krunkit.log");
    let pidfile = runtime_dir.join("krunkit.pid");
    let known_hosts = runtime_dir.join("known_hosts");

    if instance_has_running_vm(&instance.instance_dir)? {
        eprintln!("reconnecting to running vm for {}", instance.id);
        if wait_for_running_vm_ssh(
            &instance.instance_dir,
            ssh_user,
            ssh_private_key,
            &known_hosts,
            vmnet,
        )? {
            return connect_ssh(instance, ssh_user, ssh_private_key, &known_hosts, vmnet);
        }

        eprintln!(
            "running vm for {} did not accept ssh within timeout, restarting it",
            instance.id
        );
    }

    // Relaunches can leave vmnet-client orphaned even after krunkit exits.
    // Clean up any instance-owned runtime processes before reusing the same interface id.
    stop_instance_vm(&instance.instance_dir)?;
    stop_vmnet_helper_interface(&vmnet.interface_id)?;
    remove_if_exists(&known_hosts)?;

    let stdout_file = File::create(&krunkit_log).map_err(|err| err.to_string())?;
    let stderr_file = stdout_file.try_clone().map_err(|err| err.to_string())?;

    let mut command = Command::new(&vmnet.client_path);
    command
        .arg("--interface-id")
        .arg(&vmnet.interface_id)
        .arg("--operation-mode")
        .arg("shared")
        .arg("--start-address")
        .arg(&vmnet.dhcp_start)
        .arg("--end-address")
        .arg(&vmnet.dhcp_end)
        .arg("--subnet-mask")
        .arg("255.255.255.0")
        .arg("--")
        .arg("krunkit")
        .arg("--cpus")
        .arg(config.cpus.to_string())
        .arg("--memory")
        .arg(config.memory_mib.to_string())
        .arg("--pidfile")
        .arg(&pidfile)
        .arg("--device")
        .arg(format!(
            "virtio-blk,path={},format=raw",
            instance.rootfs_path.display()
        ));

    if let Some(seed_image) = &config.cloud_init_image {
        command.arg("--device").arg(format!(
            "virtio-blk,path={},format=raw",
            seed_image.display()
        ));
    }

    command
        .arg("--device")
        .arg(krunkit_network_device(&vmnet.mac_address))
        .arg("--device")
        .arg(format!(
            "virtio-fs,sharedDir={},mountTag={}",
            instance.checkout_dir.display(),
            DEFAULT_WORKSPACE_TAG
        ))
        .arg("--device")
        .arg(format!(
            "virtio-serial,logFilePath={}",
            console_log.display()
        ))
        .current_dir(&instance.checkout_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));

    let mut child = command.spawn().map_err(|err| err.to_string())?;
    let ssh_ready = wait_for_ssh(
        &mut child,
        ssh_user,
        ssh_private_key,
        &known_hosts,
        vmnet,
        &krunkit_log,
    )?;
    if !ssh_ready {
        stop_child(&mut child)?;
        return Err(format!(
            "timed out waiting for SSH on {}; see {} and {}",
            vmnet.guest_ip,
            console_log.display(),
            krunkit_log.display()
        ));
    }

    connect_ssh(instance, ssh_user, ssh_private_key, &known_hosts, vmnet)
}

fn wait_for_ssh(
    child: &mut Child,
    ssh_user: &str,
    ssh_private_key: &Path,
    known_hosts: &Path,
    vmnet: &VmnetConfig,
    krunkit_log: &Path,
) -> Result<bool, String> {
    let start = Instant::now();
    let mut progress = SshWaitProgress::new(&vmnet.guest_ip);
    while start.elapsed() < ssh_ready_timeout() {
        progress.tick(start.elapsed())?;
        if let Some(status) = child.try_wait().map_err(|err| err.to_string())? {
            progress.finish()?;
            return Err(format!(
                "vm process exited early with code {:?}",
                status.code()
            ));
        }

        if let Some(error) = detect_runtime_failure(krunkit_log)? {
            progress.finish()?;
            return Err(error);
        }

        if probe_ssh(ssh_user, ssh_private_key, known_hosts, vmnet)? {
            progress.finish()?;
            return Ok(true);
        }

        thread::sleep(Duration::from_secs(1));
    }

    progress.finish()?;
    Ok(false)
}

fn wait_for_running_vm_ssh(
    instance_dir: &Path,
    ssh_user: &str,
    ssh_private_key: &Path,
    known_hosts: &Path,
    vmnet: &VmnetConfig,
) -> Result<bool, String> {
    let start = Instant::now();
    let mut progress = SshWaitProgress::new(&vmnet.guest_ip);
    while start.elapsed() < ssh_ready_timeout() {
        progress.tick(start.elapsed())?;
        if probe_ssh(ssh_user, ssh_private_key, known_hosts, vmnet)? {
            progress.finish()?;
            return Ok(true);
        }

        if !instance_has_running_vm(instance_dir)? {
            progress.finish()?;
            return Ok(false);
        }

        thread::sleep(Duration::from_secs(1));
    }

    progress.finish()?;
    Ok(false)
}

fn detect_runtime_failure(krunkit_log: &Path) -> Result<Option<String>, String> {
    if !krunkit_log.exists() {
        return Ok(None);
    }

    let contents = fs::read_to_string(krunkit_log).map_err(|err| err.to_string())?;
    if contents.contains("vmnet_start_interface: VMNET_FAILURE") {
        return Ok(Some(format!(
            "vmnet-helper failed to create the shared network interface; see {}",
            krunkit_log.display()
        )));
    }

    Ok(None)
}

fn connect_ssh(
    instance: &Instance,
    ssh_user: &str,
    ssh_private_key: &Path,
    known_hosts: &Path,
    vmnet: &VmnetConfig,
) -> Result<i32, String> {
    let mut command = ssh_base_command(ssh_user, ssh_private_key, known_hosts, vmnet);
    for mapping in &instance.ports {
        command
            .arg("-L")
            .arg(format!("{}:127.0.0.1:{}", mapping.host, mapping.guest));
    }
    command
        .arg("-tt")
        .arg(format!("{ssh_user}@{}", vmnet.guest_ip))
        .arg(guest_shell_command());

    let status = command.status().map_err(|err| err.to_string())?;
    Ok(status.code().unwrap_or_default())
}

fn krunkit_network_device(mac_address: &str) -> String {
    format!("virtio-net,type=unixgram,fd=4,mac={mac_address}")
}

fn guest_shell_command() -> &'static str {
    "bash -lc 'if command -v cloud-init >/dev/null 2>&1; then CLOUD_INIT_STATUS=\"$(sudo cloud-init status 2>/dev/null || true)\"; if ! printf \"%s\" \"$CLOUD_INIT_STATUS\" | grep -q \"status: done\"; then echo \"waiting for cloud-init/bootstrap...\"; if sudo test -e /var/log/vibebox-init.log; then sudo sh -lc \"tail -n +1 -F /var/log/vibebox-init.log\" & TAIL_PID=$!; fi; sudo cloud-init status --wait >/dev/null 2>&1 || true; if [ -n \"${TAIL_PID:-}\" ]; then kill \"$TAIL_PID\" 2>/dev/null || true; wait \"$TAIL_PID\" 2>/dev/null || true; fi; echo \"cloud-init/bootstrap complete\"; fi; if sudo test -f /var/lib/vibebox/init.done && sudo test -f /var/log/vibebox-init.log && ! sudo test -f /var/lib/vibebox/init-log-shown; then echo \"bootstrap log:\"; sudo cat /var/log/vibebox-init.log; sudo touch /var/lib/vibebox/init-log-shown; fi; fi; ROOT_DEV=\"$(findmnt -n -o SOURCE / 2>/dev/null || true)\"; ROOT_FS=\"$(findmnt -n -o FSTYPE / 2>/dev/null || true)\"; if command -v growpart >/dev/null 2>&1 && [ -n \"$ROOT_DEV\" ]; then PARENT_DEV=\"/dev/$(lsblk -no PKNAME \"$ROOT_DEV\" 2>/dev/null || true)\"; PART_NUM=\"$(lsblk -no PARTN \"$ROOT_DEV\" 2>/dev/null || true)\"; if [ -n \"$PARENT_DEV\" ] && [ -n \"$PART_NUM\" ]; then sudo growpart \"$PARENT_DEV\" \"$PART_NUM\" >/dev/null 2>&1 || true; fi; fi; if [ -n \"$ROOT_DEV\" ]; then case \"$ROOT_FS\" in ext2|ext3|ext4) if command -v resize2fs >/dev/null 2>&1; then sudo resize2fs \"$ROOT_DEV\" >/dev/null 2>&1 || true; fi ;; xfs) if command -v xfs_growfs >/dev/null 2>&1; then sudo xfs_growfs / >/dev/null 2>&1 || true; fi ;; esac; fi; if [ -e /workspace ] && [ ! -d /workspace ]; then sudo rm -f /workspace; fi; sudo mkdir -p /workspace; if ! mountpoint -q /workspace; then sudo mount -t virtiofs workspace /workspace >/dev/null 2>&1 || true; fi; ln -sfn /workspace \"$HOME/workspace\"; cd /workspace 2>/dev/null || cd \"$HOME\"; exec /bin/bash -l'"
}

fn ssh_base_command(
    ssh_user: &str,
    ssh_private_key: &Path,
    known_hosts: &Path,
    vmnet: &VmnetConfig,
) -> Command {
    let mut command = Command::new("ssh");
    let _ = ssh_user;
    let _ = vmnet;
    command
        .arg("-i")
        .arg(ssh_private_key)
        .arg("-o")
        .arg("IdentitiesOnly=yes")
        .arg("-o")
        .arg("StrictHostKeyChecking=accept-new")
        .arg("-o")
        .arg(format!("UserKnownHostsFile={}", known_hosts.display()))
        .arg("-o")
        .arg("LogLevel=ERROR")
        .arg("-o")
        .arg("ServerAliveInterval=30")
        .arg("-o")
        .arg("ServerAliveCountMax=3");
    command
}

fn probe_ssh(
    ssh_user: &str,
    ssh_private_key: &Path,
    known_hosts: &Path,
    vmnet: &VmnetConfig,
) -> Result<bool, String> {
    let mut command = ssh_base_command(ssh_user, ssh_private_key, known_hosts, vmnet);
    command
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("ConnectionAttempts=1")
        .arg("-o")
        .arg("ConnectTimeout=2")
        .arg(format!("{ssh_user}@{}", vmnet.guest_ip))
        .arg("true")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let status = command.status().map_err(|err| err.to_string())?;
    Ok(status.success())
}

fn ssh_ready_timeout() -> Duration {
    env::var("VIBEBOX_SSH_READY_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(DEFAULT_SSH_READY_TIMEOUT_SECS))
}

struct SshWaitProgress<'a> {
    guest_ip: &'a str,
    last_reported_second: Option<u64>,
}

impl<'a> SshWaitProgress<'a> {
    fn new(guest_ip: &'a str) -> Self {
        Self {
            guest_ip,
            last_reported_second: None,
        }
    }

    fn tick(&mut self, elapsed: Duration) -> Result<(), String> {
        let elapsed_secs = elapsed.as_secs();
        if self.last_reported_second == Some(elapsed_secs) {
            return Ok(());
        }

        self.last_reported_second = Some(elapsed_secs);
        eprint!("\rwaiting for ssh on {} ({}s)", self.guest_ip, elapsed_secs);
        io::stderr().flush().map_err(|err| err.to_string())
    }

    fn finish(&mut self) -> Result<(), String> {
        if self.last_reported_second.is_some() {
            eprintln!();
            io::stderr().flush().map_err(|err| err.to_string())?;
            self.last_reported_second = None;
        }
        Ok(())
    }
}

fn stop_child(child: &mut Child) -> Result<(), String> {
    if child.try_wait().map_err(|err| err.to_string())?.is_some() {
        return Ok(());
    }
    child.kill().map_err(|err| err.to_string())?;
    let _ = child.wait().map_err(|err| err.to_string())?;
    Ok(())
}

fn stop_stale_vm(pidfile: &Path) -> Result<(), String> {
    if !pidfile.exists() {
        return Ok(());
    }

    let pid = fs::read_to_string(pidfile)
        .map_err(|err| err.to_string())?
        .trim()
        .parse::<i32>()
        .map_err(|err| format!("invalid pidfile {}: {err}", pidfile.display()))?;

    let _ = terminate_pid(pid);
    remove_if_exists(pidfile)?;
    Ok(())
}

fn instance_process_pids(instance_dir: &Path) -> Result<Vec<i32>, String> {
    let output = Command::new("ps")
        .arg("ax")
        .arg("-o")
        .arg("pid=,command=")
        .output()
        .map_err(|err| err.to_string())?;
    if !output.status.success() {
        return Err("failed to inspect process list".to_string());
    }

    Ok(parse_instance_processes(
        &String::from_utf8_lossy(&output.stdout),
        instance_dir,
    ))
}

fn instance_has_running_vm(instance_dir: &Path) -> Result<bool, String> {
    Ok(!instance_process_pids(instance_dir)?.is_empty())
}

fn vmnet_helper_pids(interface_id: &str) -> Result<Vec<i32>, String> {
    let output = Command::new("ps")
        .arg("ax")
        .arg("-o")
        .arg("pid=,command=")
        .output()
        .map_err(|err| err.to_string())?;
    if !output.status.success() {
        return Err("failed to inspect process list".to_string());
    }

    Ok(parse_vmnet_helper_processes(
        &String::from_utf8_lossy(&output.stdout),
        interface_id,
    ))
}

fn parse_instance_processes(output: &str, instance_dir: &Path) -> Vec<i32> {
    let instance_marker = instance_dir.display().to_string();
    let mut pids = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim_start();
        let Some((pid_str, command)) = trimmed.split_once(char::is_whitespace) else {
            continue;
        };
        let command = command.trim();
        if !(command.contains("krunkit") || command.contains("vmnet-client")) {
            continue;
        }
        if !command.contains(&instance_marker) {
            continue;
        }
        if let Ok(pid) = pid_str.trim().parse::<i32>() {
            pids.push(pid);
        }
    }
    pids.sort_unstable();
    pids.dedup();
    pids
}

fn parse_vmnet_helper_processes(output: &str, interface_id: &str) -> Vec<i32> {
    let mut pids = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim_start();
        let Some((pid_str, command)) = trimmed.split_once(char::is_whitespace) else {
            continue;
        };
        let command = command.trim();
        if !command.contains("vmnet-helper") {
            continue;
        }
        if !command.contains(interface_id) {
            continue;
        }
        if let Ok(pid) = pid_str.trim().parse::<i32>() {
            pids.push(pid);
        }
    }
    pids.sort_unstable();
    pids.dedup();
    pids
}

fn stop_vmnet_helper_interface(interface_id: &str) -> Result<(), String> {
    for pid in vmnet_helper_pids(interface_id)? {
        let _ = terminate_pid(pid);
    }
    Ok(())
}

fn terminate_pid(pid: i32) -> Result<(), String> {
    let term_status = Command::new("kill")
        .arg(pid.to_string())
        .stderr(Stdio::null())
        .status()
        .map_err(|err| err.to_string())?;
    if !term_status.success() {
        return Ok(());
    }

    for _ in 0..20 {
        let alive = Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stderr(Stdio::null())
            .status()
            .map_err(|err| err.to_string())?
            .success();
        if !alive {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(250));
    }

    let _ = Command::new("kill")
        .arg("-9")
        .arg(pid.to_string())
        .stderr(Stdio::null())
        .status()
        .map_err(|err| err.to_string())?;
    Ok(())
}

fn encode_env_ports(ports: &[PortMapping]) -> String {
    ports
        .iter()
        .map(|mapping| format!("{}:{}", mapping.host, mapping.guest))
        .collect::<Vec<_>>()
        .join(",")
}

fn remove_if_exists(path: &Path) -> Result<(), String> {
    if path.exists() {
        fs::remove_file(path).map_err(|err| err.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        detect_runtime_failure, encode_env_ports, guest_shell_command, krunkit_network_device,
        parse_instance_processes, parse_vmnet_helper_processes, ssh_ready_timeout,
        stop_stale_vm,
    };
    use crate::ports::PortMapping;
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use std::time::Duration;

    #[test]
    fn port_encodings_track_host_to_guest() {
        let spec = encode_env_ports(&[
            PortMapping {
                host: 21344,
                guest: 22,
            },
            PortMapping {
                host: 21345,
                guest: 3000,
            },
        ]);
        assert_eq!(spec, "21344:22,21345:3000");
    }

    #[test]
    fn krunkit_vmnet_helper_device_uses_unixgram_backend() {
        assert_eq!(
            krunkit_network_device("52:54:60:ed:a8:f9"),
            "virtio-net,type=unixgram,fd=4,mac=52:54:60:ed:a8:f9"
        );
    }

    #[test]
    fn guest_shell_waits_for_workspace_readiness() {
        let command = guest_shell_command();
        assert!(command.contains("cloud-init status --wait"));
        assert!(command.contains("waiting for cloud-init/bootstrap"));
        assert!(command.contains("/var/log/vibebox-init.log"));
        assert!(command.contains("bootstrap log:"));
        assert!(command.contains("init-log-shown"));
        assert!(command.contains("growpart"));
        assert!(command.contains("resize2fs"));
        assert!(command.contains("rm -f /workspace"));
        assert!(command.contains("mount -t virtiofs workspace /workspace"));
        assert!(command.contains("cd /workspace"));
    }

    #[test]
    fn stale_vm_cleanup_tolerates_missing_process() {
        let pidfile = PathBuf::from("/tmp/vibebox-stale-test.pid");
        fs::write(&pidfile, "999999\n").unwrap();
        stop_stale_vm(&pidfile).unwrap();
        assert!(!pidfile.exists());
    }

    #[test]
    fn process_parser_ignores_unrelated_lines() {
        let instance_dir = PathBuf::from("/tmp/instance-a");
        let output = "  10 krunkit --pidfile /tmp/instance-a/runtime/krunkit.pid\n  11 /opt/vmnet-helper/bin/vmnet-client -- /tmp/instance-a\n  12 krunkit --pidfile /tmp/instance-b/runtime/krunkit.pid\n  13 ssh something\n";
        let parsed = parse_instance_processes(output, &instance_dir);
        assert_eq!(parsed, vec![10, 11]);
    }

    #[test]
    fn vmnet_helper_parser_matches_interface_id() {
        let output = "  20 sudo --non-interactive /opt/vmnet-helper/bin/vmnet-helper --fd=3 --interface-id aaa\n  21 /opt/vmnet-helper/bin/vmnet-helper --fd=3 --interface-id aaa\n  22 /opt/vmnet-helper/bin/vmnet-helper --fd=3 --interface-id bbb\n";
        let parsed = parse_vmnet_helper_processes(output, "aaa");
        assert_eq!(parsed, vec![20, 21]);
    }

    #[test]
    fn runtime_failure_detection_reports_vmnet_errors() {
        let log_path = PathBuf::from("/tmp/vibebox-runtime-test.log");
        fs::write(
            &log_path,
            "ERROR [main] vmnet_start_interface: VMNET_FAILURE\n",
        )
        .unwrap();
        let error = detect_runtime_failure(&log_path).unwrap();
        assert!(
            error
                .unwrap()
                .contains("vmnet-helper failed to create the shared network interface")
        );
        let _ = fs::remove_file(&log_path);
    }

    #[test]
    fn ssh_ready_timeout_uses_default_when_unset_or_invalid() {
        unsafe {
            env::remove_var("VIBEBOX_SSH_READY_TIMEOUT_SECS");
        }
        assert_eq!(ssh_ready_timeout(), Duration::from_secs(300));

        unsafe {
            env::set_var("VIBEBOX_SSH_READY_TIMEOUT_SECS", "nope");
        }
        assert_eq!(ssh_ready_timeout(), Duration::from_secs(300));

        unsafe {
            env::remove_var("VIBEBOX_SSH_READY_TIMEOUT_SECS");
        }
    }

    #[test]
    fn ssh_ready_timeout_uses_env_override() {
        unsafe {
            env::set_var("VIBEBOX_SSH_READY_TIMEOUT_SECS", "45");
        }
        assert_eq!(ssh_ready_timeout(), Duration::from_secs(45));
        unsafe {
            env::remove_var("VIBEBOX_SSH_READY_TIMEOUT_SECS");
        }
    }
}
