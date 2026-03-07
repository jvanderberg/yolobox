use crate::cloud_init::{self, CloudInitOptions};
use crate::git;
use crate::network;
use crate::runtime::{self, LaunchConfig, LaunchMode};
use crate::state;
use std::env;
use std::path::PathBuf;

pub fn run() -> Result<(), String> {
    let cli = Cli::parse(env::args().skip(1).collect())?;

    match cli.command {
        Command::Base(command) => base(command),
        Command::Launch(options) => launch(options),
        Command::Status(options) => status(options),
        Command::Doctor => doctor(),
        Command::List => list_instances(),
        Command::Destroy(options) => destroy(options),
        Command::Help => {
            print_usage();
            Ok(())
        }
    }
}

enum Command {
    Base(BaseCommand),
    Launch(LaunchOptions),
    Status(TargetOptions),
    Doctor,
    List,
    Destroy(TargetOptions),
    Help,
}

enum BaseCommand {
    Import(BaseImportOptions),
    List,
}

struct Cli {
    command: Command,
}

#[derive(Clone)]
struct BaseImportOptions {
    name: String,
    image: PathBuf,
}

#[derive(Clone)]
struct LaunchOptions {
    repo: String,
    branch: String,
    new_branch: bool,
    from: Option<String>,
    base: Option<String>,
    vm: bool,
    shell_only: bool,
    no_enter: bool,
    cpus: u8,
    memory_mib: u32,
    cloud_init: bool,
    cloud_user: String,
    hostname: Option<String>,
    ssh_pubkey: Option<PathBuf>,
    ssh_private_key: Option<PathBuf>,
    init_script: Option<PathBuf>,
}

#[derive(Clone)]
struct TargetOptions {
    repo: String,
    branch: String,
}

impl Cli {
    fn parse(args: Vec<String>) -> Result<Self, String> {
        let Some((command, rest)) = args.split_first() else {
            return Ok(Self {
                command: Command::Help,
            });
        };

        let command = match command.as_str() {
            "base" => Command::Base(parse_base(rest)?),
            "launch" => Command::Launch(parse_launch(rest)?),
            "status" => Command::Status(parse_target(rest)?),
            "doctor" => Command::Doctor,
            "destroy" => Command::Destroy(parse_target(rest)?),
            "list" => Command::List,
            "help" | "--help" | "-h" => Command::Help,
            other => return Err(format!("unknown command: {other}")),
        };

        Ok(Self { command })
    }
}

fn parse_base(args: &[String]) -> Result<BaseCommand, String> {
    let Some((subcommand, rest)) = args.split_first() else {
        return Err("base requires a subcommand: import or list".to_string());
    };

    match subcommand.as_str() {
        "import" => {
            let mut name = None;
            let mut image = None;
            let mut index = 0;
            while index < rest.len() {
                match rest[index].as_str() {
                    "--name" => {
                        index += 1;
                        name = Some(expect_value(rest, index, "--name")?);
                    }
                    "--image" => {
                        index += 1;
                        image = Some(PathBuf::from(expect_value(rest, index, "--image")?));
                    }
                    flag => return Err(format!("unknown flag for base import: {flag}")),
                }
                index += 1;
            }

            Ok(BaseCommand::Import(BaseImportOptions {
                name: name.ok_or_else(|| "base import requires --name".to_string())?,
                image: image.ok_or_else(|| "base import requires --image".to_string())?,
            }))
        }
        "list" => Ok(BaseCommand::List),
        other => Err(format!("unknown base subcommand: {other}")),
    }
}

fn parse_launch(args: &[String]) -> Result<LaunchOptions, String> {
    let mut repo = None;
    let mut branch = None;
    let mut new_branch = false;
    let mut from = None;
    let mut base = None;
    let mut vm = false;
    let mut shell_only = false;
    let mut no_enter = false;
    let mut cpus = runtime::default_cpus();
    let mut memory_mib = runtime::default_memory_mib();
    let mut cloud_init = true;
    let mut cloud_user = cloud_init::default_cloud_user();
    let mut hostname = None;
    let mut ssh_pubkey = None;
    let mut ssh_private_key = None;
    let mut init_script = None;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--repo" => {
                index += 1;
                repo = Some(expect_value(args, index, "--repo")?);
            }
            "--branch" => {
                index += 1;
                branch = Some(expect_value(args, index, "--branch")?);
            }
            "--new-branch" => new_branch = true,
            "--from" => {
                index += 1;
                from = Some(expect_value(args, index, "--from")?);
            }
            "--base" => {
                index += 1;
                base = Some(expect_value(args, index, "--base")?);
            }
            "--vm" => vm = true,
            "--shell" => shell_only = true,
            "--no-enter" => no_enter = true,
            "--no-cloud-init" => cloud_init = false,
            "--cloud-user" => {
                index += 1;
                cloud_user = expect_value(args, index, "--cloud-user")?;
            }
            "--hostname" => {
                index += 1;
                hostname = Some(expect_value(args, index, "--hostname")?);
            }
            "--ssh-pubkey" => {
                index += 1;
                ssh_pubkey = Some(PathBuf::from(expect_value(args, index, "--ssh-pubkey")?));
            }
            "--ssh-private-key" => {
                index += 1;
                ssh_private_key = Some(PathBuf::from(expect_value(
                    args,
                    index,
                    "--ssh-private-key",
                )?));
            }
            "--init-script" => {
                index += 1;
                init_script = Some(PathBuf::from(expect_value(args, index, "--init-script")?));
            }
            "--cpus" => {
                index += 1;
                cpus = expect_value(args, index, "--cpus")?
                    .parse::<u8>()
                    .map_err(|_| "invalid value for --cpus".to_string())?;
            }
            "--memory-mib" => {
                index += 1;
                memory_mib = expect_value(args, index, "--memory-mib")?
                    .parse::<u32>()
                    .map_err(|_| "invalid value for --memory-mib".to_string())?;
            }
            flag => return Err(format!("unknown flag for launch: {flag}")),
        }
        index += 1;
    }

    Ok(LaunchOptions {
        repo: repo.ok_or_else(|| "launch requires --repo".to_string())?,
        branch: branch.ok_or_else(|| "launch requires --branch".to_string())?,
        new_branch,
        from,
        base,
        vm,
        shell_only,
        no_enter,
        cpus,
        memory_mib,
        cloud_init,
        cloud_user,
        hostname,
        ssh_pubkey,
        ssh_private_key,
        init_script,
    })
}

fn parse_target(args: &[String]) -> Result<TargetOptions, String> {
    let mut repo = None;
    let mut branch = None;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--repo" => {
                index += 1;
                repo = Some(expect_value(args, index, "--repo")?);
            }
            "--branch" => {
                index += 1;
                branch = Some(expect_value(args, index, "--branch")?);
            }
            flag => return Err(format!("unknown flag: {flag}")),
        }
        index += 1;
    }

    Ok(TargetOptions {
        repo: repo.ok_or_else(|| "command requires --repo".to_string())?,
        branch: branch.ok_or_else(|| "command requires --branch".to_string())?,
    })
}

fn base(command: BaseCommand) -> Result<(), String> {
    match command {
        BaseCommand::Import(options) => {
            let image = state::import_base_image(&options.name, &options.image)?;
            for line in image.summary_lines() {
                println!("{line}");
            }
            Ok(())
        }
        BaseCommand::List => {
            let images = state::list_base_images()?;
            if images.is_empty() {
                println!("no base images");
                return Ok(());
            }

            for image in images {
                println!(
                    "{}  {}  {} MiB",
                    image.name,
                    image.id,
                    image.size_bytes.div_ceil(1024 * 1024)
                );
            }
            Ok(())
        }
    }
}

fn launch(options: LaunchOptions) -> Result<(), String> {
    if options.vm && options.shell_only {
        return Err("--vm and --shell are mutually exclusive".to_string());
    }
    if !options.cloud_init && options.init_script.is_some() {
        return Err("--init-script requires cloud-init; remove --no-cloud-init".to_string());
    }

    let plan = runtime::resolve_runtime(options.shell_only);
    let instance = state::ensure_instance(&options.repo, &options.branch, options.base.as_deref())?;
    let vmnet = match plan.mode {
        LaunchMode::Krunkit => Some(network::resolve_for_instance(&instance)?),
        _ => None,
    };
    let prepared_cloud_init = match plan.mode {
        LaunchMode::External(_) | LaunchMode::Krunkit => cloud_init::prepare(
            &instance,
            &CloudInitOptions {
                enabled: options.cloud_init,
                user: options.cloud_user.clone(),
                hostname: options.hostname.clone(),
                ssh_pubkey: options.ssh_pubkey.clone(),
                init_script: options.init_script.clone(),
                network: vmnet.clone(),
            },
        )?,
        LaunchMode::Shell => None,
    };

    git::ensure_checkout(
        &instance.checkout_dir,
        &options.repo,
        &options.branch,
        options.new_branch,
        options.from.as_deref(),
    )?;

    let ssh_private_key_path = options
        .ssh_private_key
        .clone()
        .or_else(cloud_init::discover_ssh_private_key)
        .or_else(|| {
            options
                .ssh_pubkey
                .as_ref()
                .and_then(|path| cloud_init::private_key_from_public(path))
        })
        .or_else(|| {
            prepared_cloud_init
                .as_ref()
                .and_then(|prepared| cloud_init::private_key_from_public(&prepared.ssh_pubkey_path))
        });

    let launch_config = LaunchConfig {
        require_vm: !options.shell_only,
        cpus: options.cpus,
        memory_mib: options.memory_mib,
        cloud_init_image: prepared_cloud_init
            .as_ref()
            .map(|prepared| prepared.image_path.clone()),
        cloud_init_user: prepared_cloud_init
            .as_ref()
            .map(|prepared| prepared.user.clone()),
        hostname: prepared_cloud_init
            .as_ref()
            .map(|prepared| prepared.hostname.clone()),
        ssh_pubkey_path: prepared_cloud_init
            .as_ref()
            .map(|prepared| prepared.ssh_pubkey_path.clone()),
        ssh_private_key_path,
        init_script_path: prepared_cloud_init
            .as_ref()
            .and_then(|prepared| prepared.init_script_path.clone()),
        vmnet,
    };

    for line in runtime::launch_summary(&instance, &launch_config) {
        println!("{line}");
    }

    if options.no_enter {
        return Ok(());
    }

    let code = runtime::launch(plan, &instance, &launch_config)?;
    if code != 0 {
        return Err(format!("child process exited with code {code}"));
    }
    Ok(())
}

fn status(options: TargetOptions) -> Result<(), String> {
    let instance = state::find_instance(&options.repo, &options.branch)?
        .ok_or_else(|| format!("no instance found for {} {}", options.repo, options.branch))?;
    for line in instance.summary_lines() {
        println!("{line}");
    }
    Ok(())
}

fn list_instances() -> Result<(), String> {
    let instances = state::list_instances()?;
    if instances.is_empty() {
        println!("no instances");
        return Ok(());
    }

    for instance in instances {
        println!(
            "{}  {}  {}  {}",
            instance.id, instance.repo, instance.branch, instance.base_image_name
        );
    }
    Ok(())
}

fn doctor() -> Result<(), String> {
    let krunkit = runtime::command_exists("krunkit");
    let vmnet_client = network::find_vmnet_client();
    let ssh_public = cloud_init::discover_ssh_public_key();
    let ssh_private = cloud_init::discover_ssh_private_key().or_else(|| {
        ssh_public
            .as_ref()
            .and_then(|path| cloud_init::private_key_from_public(path))
    });

    println!(
        "krunkit: {}",
        if krunkit {
            "ok"
        } else {
            "missing (brew tap slp/krunkit && brew install krunkit)"
        }
    );
    println!(
        "vmnet-client: {}",
        vmnet_client
            .as_ref()
            .map(|path| format!("ok ({})", path.display()))
            .unwrap_or_else(|| {
                "missing (curl -fsSL https://raw.githubusercontent.com/nirs/vmnet-helper/main/install.sh | sudo bash)".to_string()
            })
    );
    println!(
        "ssh_pubkey: {}",
        ssh_public
            .as_ref()
            .map(|path| format!("ok ({})", path.display()))
            .unwrap_or_else(|| "missing".to_string())
    );
    println!(
        "ssh_private_key: {}",
        ssh_private
            .as_ref()
            .map(|path| format!("ok ({})", path.display()))
            .unwrap_or_else(|| "missing".to_string())
    );
    println!(
        "builtin_vm_ready: {}",
        if krunkit && vmnet_client.is_some() && ssh_public.is_some() && ssh_private.is_some() {
            "yes"
        } else {
            "no"
        }
    );
    Ok(())
}

fn destroy(options: TargetOptions) -> Result<(), String> {
    if let Some(instance) = state::find_instance(&options.repo, &options.branch)? {
        runtime::stop_instance_vm(&instance.instance_dir)?;
    }
    let removed = state::destroy_instance(&options.repo, &options.branch)?;
    match removed {
        Some(path) => println!("removed {}", path.display()),
        None => println!("nothing to remove"),
    }
    Ok(())
}

fn expect_value(args: &[String], index: usize, flag: &str) -> Result<String, String> {
    args.get(index)
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn print_usage() {
    println!("vibebox");
    println!();
    println!("Commands:");
    println!("  base import --name <base> --image <path>");
    println!("  base list");
    println!("  doctor");
    println!(
        "  launch --repo <url> --branch <name> [--base <base>] [--new-branch] [--from <base-branch>] [--shell] [--cpus <count>] [--memory-mib <size>] [--cloud-user <name>] [--hostname <name>] [--ssh-pubkey <path>] [--init-script <path>] [--no-cloud-init] [--no-enter]"
    );
    println!("  launch ... [--ssh-private-key <path>]");
    println!("  status --repo <url> --branch <name>");
    println!("  list");
    println!("  destroy --repo <url> --branch <name>");
    println!();
    println!("Environment:");
    println!("  VIBEBOX_HOME         override state directory");
    println!("  VIBEBOX_ROOTFS_MIB   override default branch disk size in MiB");
    println!("  VIBEBOX_SSH_READY_TIMEOUT_SECS  override SSH wait timeout for VM launch/reconnect");
    println!("  VIBEBOX_VM_LAUNCHER  executable invoked for libkrun handoff");
}
