//! CLI entry point for desktop sharing orchestration.

use std::io::Read;

use metis_remote::{
    accounts_list_as_root, add_input_group, add_user_as_root, apt_install, autostart_from_config,
    datetime_status_as_root, disable, enable, firewall_apply, firewall_apply_as_root,
    firewall_clear, firewall_clear_as_root, firewall_rustdesk_apply,
    firewall_rustdesk_apply_as_root, firewall_rustdesk_clear, firewall_rustdesk_clear_as_root,
    firewall_rustdesk_status, firewall_status, pause, privileged_exe, remove_user_as_root, resume,
    rustdesk_disable, rustdesk_enable, rustdesk_status, set_account_password_as_root,
    set_admin_as_root, set_display_name_as_root, set_lan_only, set_ntp_as_root, set_password,
    set_time_as_root, set_timezone_as_root, set_user_icon_as_root, status, ubuntu_drivers_install,
};
use zeroize::Zeroize;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "metis_remote=info,warn".into()),
        )
        .init();

    let code = match run(std::env::args().skip(1).collect()) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("metis-remote: {err}");
            1
        }
    };
    std::process::exit(code);
}

fn run(args: Vec<String>) -> Result<(), String> {
    match args.first().map(String::as_str) {
        None | Some("help") | Some("--help") | Some("-h") => {
            print_help();
            Ok(())
        }
        Some("status") => {
            let snap = status();
            let json = serde_json::to_string_pretty(&snap).map_err(|e| e.to_string())?;
            println!("{json}");
            Ok(())
        }
        Some("enable") => enable(),
        Some("disable") => disable(),
        Some("pause") => pause(),
        Some("resume") => resume(),
        Some("autostart") => autostart_from_config(),
        Some("set-lan-only") => {
            let flag = args
                .get(1)
                .ok_or_else(|| "usage: metis-remote set-lan-only true|false".to_string())?;
            let on = match flag.as_str() {
                "true" | "1" | "on" | "yes" => true,
                "false" | "0" | "off" | "no" => false,
                other => {
                    return Err(format!(
                        "invalid lan_only value '{other}' (use true or false)"
                    ));
                }
            };
            set_lan_only(on)
        }
        Some("set-credentials") => {
            let username = args
                .get(1)
                .cloned()
                .or_else(|| std::env::var("USER").ok())
                .ok_or_else(|| {
                    "usage: metis-remote set-credentials <username>  (password on stdin)"
                        .to_string()
                })?;
            let mut password = String::new();
            std::io::stdin()
                .read_to_string(&mut password)
                .map_err(|e| format!("read password from stdin: {e}"))?;
            // Accept a single line; ignore trailing newline / CR.
            let trimmed = password.trim_end_matches(['\r', '\n']);
            if trimmed.is_empty() {
                password.zeroize();
                return Err(String::from("password on stdin must not be empty"));
            }
            let mut owned = trimmed.to_string();
            password.zeroize();
            let result = set_password(&username, &owned);
            owned.zeroize();
            result
        }
        Some("firewall") => match args.get(1).map(String::as_str) {
            Some("apply") => {
                let snap = firewall_apply()?;
                print_firewall(&snap)
            }
            Some("clear") => {
                let snap = firewall_clear()?;
                print_firewall(&snap)
            }
            Some("status") => {
                let snap = firewall_status();
                print_firewall(&snap)
            }
            Some("apply-as-root") => {
                let snap = firewall_apply_as_root()?;
                print_firewall(&snap)
            }
            Some("clear-as-root") => {
                let snap = firewall_clear_as_root()?;
                print_firewall(&snap)
            }
            Some("rustdesk-apply") => {
                let snap = firewall_rustdesk_apply()?;
                print_firewall(&snap)
            }
            Some("rustdesk-clear") => {
                let snap = firewall_rustdesk_clear()?;
                print_firewall(&snap)
            }
            Some("rustdesk-status") => {
                let snap = firewall_rustdesk_status();
                print_firewall(&snap)
            }
            Some("rustdesk-apply-as-root") => {
                let snap = firewall_rustdesk_apply_as_root()?;
                print_firewall(&snap)
            }
            Some("rustdesk-clear-as-root") => {
                let snap = firewall_rustdesk_clear_as_root()?;
                print_firewall(&snap)
            }
            _ => Err(
                "usage: metis-remote firewall {apply|clear|status|apply-as-root|clear-as-root|\
                 rustdesk-apply|rustdesk-clear|rustdesk-status|rustdesk-apply-as-root|rustdesk-clear-as-root}"
                    .into(),
            ),
        },
        Some("rustdesk") => match args.get(1).map(String::as_str) {
            Some("status") => {
                let snap = rustdesk_status();
                let json = serde_json::to_string_pretty(&snap).map_err(|e| e.to_string())?;
                println!("{json}");
                Ok(())
            }
            Some("enable") => rustdesk_enable(),
            Some("disable") => {
                let kill = args.get(2).map(String::as_str) == Some("--kill");
                rustdesk_disable(kill)
            }
            _ => Err(String::from("usage: metis-remote rustdesk {status|enable|disable [--kill]}")),
        },
        Some("pk-apt-install") => {
            let pkgs: Vec<String> = args.into_iter().skip(1).collect();
            apt_install(&pkgs)
        }
        Some("pk-add-input-group") => {
            let user = args
                .get(1)
                .cloned()
                .ok_or_else(|| "usage: metis-remote pk-add-input-group <username>".to_string())?;
            add_input_group(&user)
        }
        Some("pk-ubuntu-drivers-install") => ubuntu_drivers_install(),
        Some("pk-accounts-list") => accounts_list_as_root(),
        Some("pk-accounts-set-name") => {
            let user = args
                .get(1)
                .cloned()
                .ok_or_else(|| String::from("usage: metis-remote pk-accounts-set-name <user> <name>"))?;
            let name = args
                .get(2)
                .cloned()
                .ok_or_else(|| String::from("usage: metis-remote pk-accounts-set-name <user> <name>"))?;
            set_display_name_as_root(&user, &name)
        }
        Some("pk-accounts-set-icon") => {
            let user = args
                .get(1)
                .cloned()
                .ok_or_else(|| String::from("usage: metis-remote pk-accounts-set-icon <user> <path>"))?;
            let path = args
                .get(2)
                .cloned()
                .ok_or_else(|| String::from("usage: metis-remote pk-accounts-set-icon <user> <path>"))?;
            set_user_icon_as_root(&user, &path)
        }
        Some("pk-accounts-set-password") => {
            let user = args.get(1).cloned().ok_or_else(|| {
                String::from(
                    "usage: metis-remote pk-accounts-set-password <user> [--password-file PATH]",
                )
            })?;
            let password_file = parse_password_file_arg(&args[2..])?;
            set_account_password_as_root(&user, password_file.as_deref())
        }
        Some("pk-accounts-set-admin") => {
            let user = args
                .get(1)
                .cloned()
                .ok_or_else(|| String::from("usage: metis-remote pk-accounts-set-admin <user> true|false"))?;
            let flag = args
                .get(2)
                .ok_or_else(|| String::from("usage: metis-remote pk-accounts-set-admin <user> true|false"))?;
            let on = match flag.as_str() {
                "true" | "1" | "yes" | "on" => true,
                "false" | "0" | "no" | "off" => false,
                other => return Err(format!("invalid admin flag '{other}'")),
            };
            set_admin_as_root(&user, on)
        }
        Some("pk-accounts-add") => {
            let user = args.get(1).cloned().ok_or_else(|| {
                String::from(
                    "usage: metis-remote pk-accounts-add <user> [--admin] [--name N] [--password-file PATH]",
                )
            })?;
            let mut admin = false;
            let mut display: Option<String> = None;
            let mut password_file: Option<String> = None;
            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "--admin" => {
                        admin = true;
                        i += 1;
                    }
                    "--name" => {
                        let n = args.get(i + 1).cloned().ok_or_else(|| {
                            String::from(
                                "usage: metis-remote pk-accounts-add <user> [--admin] [--name N] [--password-file PATH]",
                            )
                        })?;
                        display = Some(n);
                        i += 2;
                    }
                    "--password-file" => {
                        let p = args.get(i + 1).cloned().ok_or_else(|| {
                            String::from(
                                "usage: metis-remote pk-accounts-add … --password-file PATH",
                            )
                        })?;
                        password_file = Some(p);
                        i += 2;
                    }
                    other => return Err(format!("unknown pk-accounts-add flag '{other}'")),
                }
            }
            add_user_as_root(&user, display.as_deref(), admin, password_file.as_deref())
        }
        Some("pk-accounts-remove") => {
            let user = args
                .get(1)
                .cloned()
                .ok_or_else(|| String::from("usage: metis-remote pk-accounts-remove <user>"))?;
            remove_user_as_root(&user)
        }
        Some("pk-datetime-status") => datetime_status_as_root(),
        Some("pk-datetime-set-ntp") => {
            let flag = args
                .get(1)
                .ok_or_else(|| String::from("usage: metis-remote pk-datetime-set-ntp true|false"))?;
            let on = match flag.as_str() {
                "true" | "1" | "yes" | "on" => true,
                "false" | "0" | "no" | "off" => false,
                other => return Err(format!("invalid ntp flag '{other}'")),
            };
            set_ntp_as_root(on)
        }
        Some("pk-datetime-set-timezone") => {
            let tz = args
                .get(1)
                .cloned()
                .ok_or_else(|| String::from("usage: metis-remote pk-datetime-set-timezone <Area/City>"))?;
            set_timezone_as_root(&tz)
        }
        Some("pk-datetime-set-time") => {
            let spec = args.get(1..).map(|s| s.join(" ")).filter(|s| !s.is_empty()).ok_or_else(
                || String::from("usage: metis-remote pk-datetime-set-time <YYYY-MM-DD HH:MM:SS>"),
            )?;
            set_time_as_root(&spec)
        }
        // Dev helper: show which binary pkexec would use.
        Some("pk-exe") => {
            println!("{}", privileged_exe().display());
            Ok(())
        }
        Some(cmd) => Err(format!("unknown command: {cmd}")),
    }
}

fn print_firewall(snap: &metis_remote::FirewallStatus) -> Result<(), String> {
    let json = serde_json::to_string_pretty(snap).map_err(|e| e.to_string())?;
    println!("{json}");
    Ok(())
}

fn parse_password_file_arg(args: &[String]) -> Result<Option<String>, String> {
    let mut out = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--password-file" => {
                let p = args
                    .get(i + 1)
                    .cloned()
                    .ok_or_else(|| String::from("missing path after --password-file"))?;
                out = Some(p);
                i += 2;
            }
            other => return Err(format!("unexpected argument '{other}'")),
        }
    }
    Ok(out)
}

fn print_help() {
    eprintln!(
        "Usage: metis-remote <command>

  status              Print JSON status (for Settings UI)
  enable              Start session-sharing RDP per remote.json
  disable             Stop RDP, clear enabled flag, clear LAN firewall rules
  pause               Stop RDP listen (keep remote.json enabled) — used on lock
  resume              Re-enable RDP if remote.json still enabled — used on unlock
  autostart           Enable sharing when remote.json enabled + auto_start
  set-credentials U   Set RDP login; password is read from stdin (one line)
  set-lan-only BOOL   Persist lan_only and apply/clear firewall when sharing is on
  firewall apply      Apply LAN-only rules for TCP 3389 (pkexec if needed)
  firewall clear      Remove Metis LAN-only rules
  firewall status     Print firewall helper status JSON
  rustdesk status     Print RustDesk install/running JSON
  rustdesk enable     Start RustDesk + optional LAN firewall (GRD stays default host)
  rustdesk disable    Clear RustDesk backend preference ([--kill] stops process)
  pk-apt-install …    Polkit: install allowlisted apt packages
  pk-ubuntu-drivers-install  Polkit: ubuntu-drivers install (NVIDIA consent path)
  pk-accounts-list / set-name / set-password / set-admin / set-icon / add / remove
  pk-datetime-status / set-ntp / set-timezone / set-time

Never put the RDP password on the shell command line — pipe it to stdin."
    );
}
