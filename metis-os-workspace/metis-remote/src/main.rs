//! CLI entry point for desktop sharing orchestration.

use std::io::Read;

use metis_remote::{
    add_input_group, apt_install, autostart_from_config, disable, enable, firewall_apply,
    firewall_apply_as_root, firewall_clear, firewall_clear_as_root, firewall_rustdesk_apply,
    firewall_rustdesk_apply_as_root, firewall_rustdesk_clear, firewall_rustdesk_clear_as_root,
    firewall_rustdesk_status, firewall_status, pause, privileged_exe, resume, rustdesk_disable,
    rustdesk_enable, rustdesk_status, set_lan_only, set_password, status, ubuntu_drivers_install,
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
                return Err("password on stdin must not be empty".into());
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
            _ => Err("usage: metis-remote rustdesk {status|enable|disable [--kill]}".into()),
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

Never put the RDP password on the shell command line — pipe it to stdin."
    );
}
