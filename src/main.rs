mod client;
mod commands;
mod config;
mod credential_pipe;
mod disclosure;
mod error;
mod models;
mod output;
mod resolve;

use clap::{Parser, Subcommand};
use client::InfisicalClient;
use colored::Colorize;
use config::Config;
use error::Result;
use output::Format;

#[derive(Parser)]
#[command(
    name = "inf",
    version,
    about = "Infisical control-plane CLI for agents - projects, environments and machine identities",
    long_about = "inf manages the Infisical resources the official `infisical` CLI cannot: \
                  projects, environments and machine identities.\n\n\
                  It is built to be driven by agents and scripts: every command writes JSON \
                  to stdout and nothing else, and errors are JSON on stderr. Pass --human for \
                  tables and colour when a person is reading.\n\n\
                  It reuses the session established by `infisical login`, so no separate \
                  credentials are needed. Secret read/write stays with the official CLI."
)]
struct Cli {
    /// Infisical instance URL. Defaults to $INFISICAL_DOMAIN, then the instance
    /// `infisical login` last used.
    #[arg(long, global = true)]
    domain: Option<String>,

    /// Render tables and status lines for a human reader instead of JSON.
    /// Output is JSON unless this is passed.
    #[arg(long, global = true)]
    human: bool,

    /// Display real credentials (requires exposure acknowledgement).
    #[arg(long, global = true)]
    reveal: bool,

    /// Acknowledge that displayed credentials can persist in agent context/history.
    #[arg(long, global = true)]
    acknowledge_secret_exposure: bool,

    /// Deliver issued credentials to a new owner-private file.
    #[arg(long, global = true)]
    output_file: Option<std::path::PathBuf>,

    /// Deliver issued credentials to an inherited private descriptor >= 3.
    #[arg(long, global = true)]
    output_fd: Option<i32>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show the active session: instance, organization and expiry
    Whoami,
    /// Manage projects and their environments
    #[command(subcommand)]
    Project(ProjectCommands),
    /// Manage machine identities
    #[command(subcommand, visible_alias = "id")]
    Identity(IdentityCommands),
    /// Manage SSH certificate authorities and certificate templates
    #[command(subcommand)]
    Ssh(SshCommands),
}

#[derive(Subcommand)]
enum ProjectCommands {
    /// List projects
    #[command(visible_alias = "list")]
    Ls,
    /// Show one project and its environments
    Get {
        /// Project id, slug, name, or a unique substring
        query: String,
    },
    /// Create a project
    Create {
        /// Project name
        #[arg(long)]
        name: String,
        /// URL-safe slug. The server generates one from the name — with a
        /// random suffix — when this is omitted.
        #[arg(long)]
        slug: Option<String>,
        #[arg(long)]
        description: Option<String>,
        /// Product this project belongs to. `ssh` and `ai` were removed in
        /// Infisical v0.162.16 and are rejected by servers at or above it.
        #[arg(long, default_value = "secret-manager",
              value_parser = ["secret-manager", "cert-manager", "kms", "ssh", "secret-scanning", "pam", "ai"])]
        r#type: String,
        /// Do not create Development, Staging and Production environments
        #[arg(long)]
        no_default_envs: bool,
        /// Block deletion immediately
        #[arg(long)]
        delete_protection: bool,
    },
    /// Rename a project
    Rename {
        /// Project id, slug, name, or a unique substring
        query: String,
        /// New name
        new_name: String,
    },
    /// Update project fields
    Update {
        /// Project id, slug, name, or a unique substring
        query: String,
        #[arg(long)]
        name: Option<String>,
        /// Change the slug. Breaks anything referencing the old one.
        #[arg(long)]
        slug: Option<String>,
        #[arg(long)]
        description: Option<String>,
        /// Block deletion until explicitly disabled
        #[arg(long, conflicts_with = "no_delete_protection")]
        delete_protection: bool,
        /// Allow deletion
        #[arg(long)]
        no_delete_protection: bool,
    },
    /// Delete a project and every secret in it
    #[command(visible_alias = "rm")]
    Delete {
        /// Project id, slug, name, or a unique substring
        query: String,
        /// Proceed without confirmation. Required outside --human.
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// List machine identities with access to a project
    Identities {
        /// Project id, slug, name, or a unique substring
        query: String,
    },
    /// Manage environments within a project
    #[command(subcommand)]
    Env(EnvCommands),
}

#[derive(Subcommand)]
enum EnvCommands {
    /// List environments in a project
    #[command(visible_alias = "list")]
    Ls {
        /// Project id, slug, name, or a unique substring
        project: String,
    },
    /// Create an environment
    Create {
        /// Project id, slug, name, or a unique substring
        project: String,
        /// Environment name
        #[arg(long)]
        name: String,
        /// Slug used in secret paths. Derived from the name when omitted.
        #[arg(long)]
        slug: Option<String>,
    },
    /// Rename an environment or change its slug
    Update {
        /// Project id, slug, name, or a unique substring
        project: String,
        /// Environment slug, id or name
        env: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        slug: Option<String>,
    },
    /// Delete an environment and its secrets
    #[command(visible_alias = "rm")]
    Delete {
        /// Project id, slug, name, or a unique substring
        project: String,
        /// Environment slug, id or name
        env: String,
        /// Proceed without confirmation. Required outside --human.
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum IdentityCommands {
    /// List machine identities
    ///
    /// Without --project this lists organization-level identities only.
    /// Project-scoped identities never appear there, so pass --project to see
    /// the ones inf creates.
    #[command(visible_alias = "list")]
    Ls {
        /// Project id, slug, name, or a unique substring
        #[arg(long)]
        project: Option<String>,
    },
    /// Show one identity
    Get {
        /// Identity id, name, or a unique substring
        query: String,
        /// Project the identity is scoped to
        #[arg(long)]
        project: Option<String>,
    },
    /// Create a machine identity scoped to a single project
    ///
    /// Identities are always project-scoped. inf provides no way to create an
    /// organization-level identity, because such an identity is visible
    /// org-wide and can be granted access to any project in the organization.
    Create {
        /// Identity name
        #[arg(long)]
        name: String,
        /// Project the identity is confined to. Id, slug, name or a unique
        /// substring.
        #[arg(long)]
        project: String,
        /// Project role. Defaults to read-only. Custom role slugs are accepted,
        /// so this is validated by the server rather than here.
        #[arg(long, default_value = "viewer")]
        role: String,
        /// Also attach Universal Auth and mint a client secret, so the
        /// identity is immediately usable
        #[arg(long)]
        with_universal_auth: bool,
        /// Attach the locked-down 900-second Universal Auth profile used by
        /// SSH certificate issuers, then mint one client secret
        #[arg(long, conflicts_with = "with_universal_auth")]
        bounded_ssh_universal_auth: bool,
    },
    /// Rename an identity
    Rename {
        /// Identity id, name, or a unique substring
        query: String,
        /// New name
        new_name: String,
        /// Project the identity is scoped to
        #[arg(long)]
        project: Option<String>,
    },
    /// Change an identity's role within its project
    SetRole {
        /// Identity id, name, or a unique substring
        query: String,
        /// Project the identity is scoped to
        #[arg(long)]
        project: String,
        /// Project role. Defaults to read-only.
        #[arg(long, default_value = "viewer")]
        role: String,
    },
    /// Delete an identity
    #[command(visible_alias = "rm")]
    Delete {
        /// Identity id, name, or a unique substring
        query: String,
        /// Project the identity is scoped to
        #[arg(long)]
        project: Option<String>,
        /// Proceed without confirmation. Required outside --human.
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Mint a Universal Auth client secret, attaching the method if needed
    ClientSecret {
        /// Identity id, name, or a unique substring
        query: String,
        /// Project the identity is scoped to
        #[arg(long)]
        project: Option<String>,
        #[arg(long, default_value = "created by inf")]
        description: String,
        /// Lifetime in seconds; 0 means no expiry
        #[arg(long, default_value_t = 0)]
        ttl: i64,
        /// Maximum number of uses; 0 means unlimited
        #[arg(long, default_value_t = 0)]
        num_uses: i64,
    },
    /// Attach the locked-down 900-second SSH issuer auth profile without
    /// creating a client secret
    AttachBoundedSshAuth {
        /// Identity id, name, or a unique substring
        query: String,
        /// Project the identity is scoped to
        #[arg(long)]
        project: String,
    },
}

#[derive(Subcommand)]
enum SshCommands {
    /// Manage SSH certificate authorities
    #[command(subcommand)]
    Ca(SshCaCommands),
    /// Manage SSH User-certificate templates
    #[command(subcommand)]
    Template(SshTemplateCommands),
    /// Manage the fixed one-permission certificate issuer role
    #[command(subcommand)]
    IssuerRole(SshIssuerRoleCommands),
}

#[derive(Subcommand)]
enum SshIssuerRoleCommands {
    /// List project roles for an SSH project
    #[command(visible_alias = "list")]
    Ls { project: String },
    /// Create a role that can only issue SSH certificates
    Create {
        project: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        slug: String,
    },
}

#[derive(Subcommand)]
enum SshCaCommands {
    /// List the User and Host CAs in an SSH project
    #[command(visible_alias = "list")]
    Ls {
        /// SSH project id, slug, name, or a unique substring
        project: String,
    },
    /// Rename a CA and set its status
    Update {
        /// SSH project id, slug, name, or a unique substring
        project: String,
        /// CA id, friendly name, or a unique substring
        ca: String,
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "active", value_parser = ["active", "disabled"])]
        status: String,
    },
    /// Return a CA public key
    PublicKey {
        /// SSH project id, slug, name, or a unique substring
        project: String,
        /// CA id, friendly name, or a unique substring
        ca: String,
    },
}

#[derive(Subcommand)]
enum SshTemplateCommands {
    /// List User-certificate templates in an SSH project
    #[command(visible_alias = "list")]
    Ls {
        /// SSH project id, slug, name, or a unique substring
        project: String,
    },
    /// Create a constrained User-certificate template
    Create {
        /// SSH project id, slug, name, or a unique substring
        project: String,
        /// CA id, friendly name, or a unique substring
        #[arg(long, default_value = "User CA")]
        ca: String,
        #[arg(long)]
        name: String,
        /// Allowed certificate principal. Repeat for more than one.
        #[arg(long, required = true)]
        principal: Vec<String>,
        #[arg(long, default_value = "30m")]
        ttl: String,
        #[arg(long, default_value = "30m")]
        max_ttl: String,
    },
}

/// Parse, rendering clap's own failures through the JSON envelope.
///
/// `Cli::parse()` printed English usage text and exited before any of the JSON
/// machinery ran, so an agent that generated a slightly wrong command line got
/// an unparseable failure. `--help` and `--version` are the deliberate
/// exception: they are human-invoked, exit 0, and keep clap's rendering.
fn parse_or_exit() -> Cli {
    match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            use clap::error::ErrorKind;
            if matches!(
                err.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                let _ = err.print();
                std::process::exit(0);
            }
            // `--human` may not have parsed, so decide the mode from argv.
            let human = std::env::args().any(|a| a == "--human");
            let error = error::IfpError::Usage(err.to_string().trim_end().to_string());
            report(&error, Format::new(human));
            std::process::exit(error.exit_code());
        }
    }
}

/// stdout stays reserved for the result, so failures go to stderr — as JSON by
/// default, because the caller is normally a program that has to branch on the
/// failure rather than read it.
fn report(err: &error::IfpError, format: Format) {
    if format.is_human() {
        eprintln!("{} {err}", "error:".red().bold());
    } else {
        eprintln!(
            "{}",
            serde_json::to_string_pretty(&err.to_json()).unwrap_or_default()
        );
    }
}

/// Restore the default `SIGPIPE` disposition.
///
/// Rust sets `SIGPIPE` to `SIG_IGN` at startup, which turns a closed stdout
/// into an `EPIPE` that `println!` responds to by panicking — exit 101, with a
/// Rust backtrace on stderr. `inf project ls | head -1` is an ordinary agent
/// invocation, and it would produce both an exit code outside the documented
/// set and a stderr that is not JSON. Dying from `SIGPIPE` like every other
/// Unix tool is the honest behaviour.
#[cfg(unix)]
fn restore_sigpipe() {
    // Safe: called from `main` before the Tokio runtime is constructed, so the
    // process is still single-threaded and nothing else can be handling
    // signals concurrently.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
fn restore_sigpipe() {}

/// Deliberately a *synchronous* `main`. `#[tokio::main]` expands to a runtime
/// built before the async body runs, so restoring the signal disposition
/// inside that body would happen after worker threads already exist.
fn main() {
    restore_sigpipe();
    run_cli();
}

#[tokio::main]
async fn run_cli() {
    let cli = parse_or_exit();
    let format = Format::new(cli.human);

    if let Err(err) = run(cli).await {
        report(&err, format);
        std::process::exit(err.exit_code());
    }
}

async fn run(cli: Cli) -> Result<()> {
    let format = Format::new(cli.human);
    let issues_credential = matches!(
        &cli.command,
        Commands::Identity(IdentityCommands::ClientSecret { .. })
            | Commands::Identity(IdentityCommands::Create {
                with_universal_auth: true,
                ..
            })
            | Commands::Identity(IdentityCommands::Create {
                bounded_ssh_universal_auth: true,
                ..
            })
    );
    disclosure::prepare(
        cli.reveal,
        cli.acknowledge_secret_exposure,
        cli.output_file.as_deref(),
        cli.output_fd,
        issues_credential,
    )?;
    let config = Config::load(cli.domain.as_deref()).await?;

    // whoami is answered entirely from the decoded session, so it stays useful
    // when the API is unreachable.
    if let Commands::Whoami = &cli.command {
        return whoami(&config, format);
    }

    let client = InfisicalClient::new(config)?;

    match cli.command {
        Commands::Whoami => unreachable!("handled above"),

        Commands::Project(command) => match command {
            ProjectCommands::Ls => commands::project::list(&client, format).await,
            ProjectCommands::Get { query } => commands::project::get(&client, &query, format).await,
            ProjectCommands::Create {
                name,
                slug,
                description,
                r#type,
                no_default_envs,
                delete_protection,
            } => {
                commands::project::create(
                    &client,
                    &name,
                    slug.as_deref(),
                    description.as_deref(),
                    &r#type,
                    !no_default_envs,
                    delete_protection,
                    format,
                )
                .await
            }
            ProjectCommands::Rename { query, new_name } => {
                commands::project::rename(&client, &query, &new_name, format).await
            }
            ProjectCommands::Update {
                query,
                name,
                slug,
                description,
                delete_protection,
                no_delete_protection,
            } => {
                // Two flags model one tri-state: set, cleared, or untouched.
                let protection = match (delete_protection, no_delete_protection) {
                    (true, false) => Some(true),
                    (false, true) => Some(false),
                    _ => None,
                };
                commands::project::update(
                    &client,
                    &query,
                    name.as_deref(),
                    slug.as_deref(),
                    description.as_deref(),
                    protection,
                    format,
                )
                .await
            }
            ProjectCommands::Delete { query, yes } => {
                commands::project::delete(&client, &query, yes, format).await
            }
            ProjectCommands::Identities { query } => {
                commands::project::identities(&client, &query, format).await
            }
            ProjectCommands::Env(command) => match command {
                EnvCommands::Ls { project } => {
                    commands::project::env_list(&client, &project, format).await
                }
                EnvCommands::Create {
                    project,
                    name,
                    slug,
                } => {
                    commands::project::env_create(&client, &project, &name, slug.as_deref(), format)
                        .await
                }
                EnvCommands::Update {
                    project,
                    env,
                    name,
                    slug,
                } => {
                    commands::project::env_update(
                        &client,
                        &project,
                        &env,
                        name.as_deref(),
                        slug.as_deref(),
                        format,
                    )
                    .await
                }
                EnvCommands::Delete { project, env, yes } => {
                    commands::project::env_delete(&client, &project, &env, yes, format).await
                }
            },
        },

        Commands::Identity(command) => match command {
            IdentityCommands::Ls { project } => {
                commands::identity::list(&client, project.as_deref(), format).await
            }
            IdentityCommands::Get { query, project } => {
                commands::identity::get(&client, &query, project.as_deref(), format).await
            }
            IdentityCommands::Create {
                name,
                project,
                role,
                with_universal_auth,
                bounded_ssh_universal_auth,
            } => {
                commands::identity::create(
                    &client,
                    &name,
                    &project,
                    &role,
                    with_universal_auth,
                    bounded_ssh_universal_auth,
                    format,
                )
                .await
            }
            IdentityCommands::Rename {
                query,
                new_name,
                project,
            } => {
                commands::identity::rename(&client, &query, &new_name, project.as_deref(), format)
                    .await
            }
            IdentityCommands::SetRole {
                query,
                project,
                role,
            } => commands::identity::set_role(&client, &query, &project, &role, format).await,
            IdentityCommands::Delete {
                query,
                project,
                yes,
            } => commands::identity::delete(&client, &query, project.as_deref(), yes, format).await,
            IdentityCommands::ClientSecret {
                query,
                project,
                description,
                ttl,
                num_uses,
            } => {
                commands::identity::client_secret(
                    &client,
                    &query,
                    project.as_deref(),
                    &description,
                    ttl,
                    num_uses,
                    format,
                )
                .await
            }
            IdentityCommands::AttachBoundedSshAuth { query, project } => {
                commands::identity::attach_bounded_ssh_auth(&client, &query, &project, format).await
            }
        },

        Commands::Ssh(command) => match command {
            SshCommands::Ca(command) => match command {
                SshCaCommands::Ls { project } => {
                    commands::ssh::ca_list(&client, &project, format).await
                }
                SshCaCommands::Update {
                    project,
                    ca,
                    name,
                    status,
                } => commands::ssh::ca_update(&client, &project, &ca, &name, &status, format).await,
                SshCaCommands::PublicKey { project, ca } => {
                    commands::ssh::ca_public_key(&client, &project, &ca, format).await
                }
            },
            SshCommands::Template(command) => match command {
                SshTemplateCommands::Ls { project } => {
                    commands::ssh::template_list(&client, &project, format).await
                }
                SshTemplateCommands::Create {
                    project,
                    ca,
                    name,
                    principal,
                    ttl,
                    max_ttl,
                } => {
                    commands::ssh::template_create(
                        &client, &project, &ca, &name, &principal, &ttl, &max_ttl, format,
                    )
                    .await
                }
            },
            SshCommands::IssuerRole(command) => match command {
                SshIssuerRoleCommands::Ls { project } => {
                    commands::ssh::issuer_role_list(&client, &project, format).await
                }
                SshIssuerRoleCommands::Create {
                    project,
                    name,
                    slug,
                } => {
                    commands::ssh::issuer_role_create(&client, &project, &name, &slug, format).await
                }
            },
        },
    }
}

fn whoami(config: &Config, format: Format) -> Result<()> {
    let days = config.days_until_expiry();

    output::emit(
        format,
        &serde_json::json!({
            "domain": config.api_url,
            "organizationId": config.org_id,
            "expiresAt": config.expires_at,
            "daysUntilExpiry": days,
        }),
        || {
            println!("{} {}", "Instance:".bold(), config.api_url);
            println!("{} {}", "Organization:".bold(), config.org_id);
            match days {
                Some(days) if days <= 1 => {
                    println!("{} {}", "Session:".bold(), "expires within a day".yellow());
                }
                Some(days) => println!("{} valid for {} more days", "Session:".bold(), days),
                None => println!("{} valid", "Session:".bold()),
            }
        },
    );
    if matches!(days, Some(days) if days <= 1) {
        output::warn(
            format,
            "run `infisical login` soon; the CLI cannot refresh this session",
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// clap's own consistency checks: duplicate flags, bad `conflicts_with`
    /// targets, and ids that do not exist all surface here rather than at
    /// runtime.
    #[test]
    fn cli_definition_is_internally_consistent() {
        Cli::command().debug_assert();
    }

    fn parse(args: &[&str]) -> std::result::Result<Cli, clap::Error> {
        Cli::try_parse_from(args)
    }

    fn parses(args: &[&str]) -> Cli {
        parse(args).unwrap_or_else(|e| panic!("failed to parse {args:?}: {e}"))
    }

    /// Every invocation documented in README.md. These exist so the CLI surface
    /// can be verified offline — checking it by running the binary means
    /// mutating whatever instance the caller happens to be logged into.
    #[test]
    fn documented_invocations_parse() {
        for args in [
            vec!["inf", "whoami"],
            vec!["inf", "project", "ls"],
            vec!["inf", "project", "get", "billing"],
            vec![
                "inf",
                "project",
                "create",
                "--name",
                "Billing API",
                "--description",
                "invoicing",
            ],
            vec!["inf", "project", "rename", "billing", "Payments API"],
            vec![
                "inf",
                "project",
                "update",
                "billing",
                "--slug",
                "payments-api",
            ],
            vec!["inf", "project", "delete", "billing", "--yes"],
            vec!["inf", "project", "identities", "billing"],
            vec!["inf", "project", "env", "ls", "billing"],
            vec![
                "inf", "project", "env", "create", "billing", "--name", "QA (EU)",
            ],
            vec![
                "inf",
                "project",
                "env",
                "update",
                "billing",
                "qa-eu",
                "--name",
                "QA Europe",
            ],
            vec![
                "inf", "project", "env", "delete", "billing", "qa-eu", "--yes",
            ],
            vec![
                "inf",
                "identity",
                "create",
                "--name",
                "ci-reader",
                "--project",
                "billing",
                "--with-universal-auth",
            ],
            vec!["inf", "identity", "ls", "--project", "billing"],
            vec![
                "inf",
                "identity",
                "get",
                "ci-reader",
                "--project",
                "billing",
            ],
            vec![
                "inf",
                "identity",
                "set-role",
                "ci-reader",
                "--project",
                "billing",
                "--role",
                "viewer",
            ],
            vec![
                "inf",
                "identity",
                "rename",
                "ci-reader",
                "billing-reader",
                "--project",
                "billing",
            ],
            vec![
                "inf",
                "identity",
                "client-secret",
                "ci-reader",
                "--project",
                "billing",
                "--ttl",
                "3600",
            ],
            vec![
                "inf",
                "identity",
                "delete",
                "ci-reader",
                "--project",
                "billing",
                "--yes",
            ],
        ] {
            parses(&args);
        }
    }

    #[test]
    fn documented_aliases_parse() {
        for args in [
            vec!["inf", "project", "list"],
            vec!["inf", "project", "rm", "billing", "--yes"],
            vec!["inf", "id", "list"],
            vec!["inf", "identity", "rm", "ci", "--yes"],
            vec!["inf", "project", "env", "rm", "billing", "dev", "--yes"],
        ] {
            parses(&args);
        }
    }

    #[test]
    fn json_is_the_default_and_human_is_opt_in() {
        assert!(!parses(&["inf", "project", "ls"]).human);
        assert!(parses(&["inf", "--human", "project", "ls"]).human);
        // Global, so it is accepted after the subcommand too.
        assert!(parses(&["inf", "project", "ls", "--human"]).human);
    }

    #[test]
    fn there_is_no_json_flag_because_json_is_the_default() {
        assert!(parse(&["inf", "project", "ls", "--json"]).is_err());
        assert!(parse(&["inf", "identity", "get", "ci", "--json"]).is_err());
    }

    #[test]
    fn domain_is_global() {
        let cli = parses(&["inf", "project", "ls", "--domain", "https://ex.com"]);
        assert_eq!(cli.domain.as_deref(), Some("https://ex.com"));
    }

    #[test]
    fn delete_protection_flags_conflict() {
        // The pair models one tri-state; accepting both would silently pick one.
        assert!(parse(&[
            "inf",
            "project",
            "update",
            "billing",
            "--delete-protection",
            "--no-delete-protection",
        ])
        .is_err());
    }

    #[test]
    fn documented_defaults_hold() {
        let cli = parses(&[
            "inf",
            "identity",
            "create",
            "--name",
            "ci",
            "--project",
            "p",
        ]);
        match cli.command {
            Commands::Identity(IdentityCommands::Create {
                role,
                with_universal_auth,
                ..
            }) => {
                assert_eq!(role, "viewer", "identities must default to read-only");
                assert!(!with_universal_auth);
            }
            _ => panic!("wrong subcommand"),
        }

        let cli = parses(&["inf", "project", "create", "--name", "p"]);
        match cli.command {
            Commands::Project(ProjectCommands::Create { r#type, slug, .. }) => {
                assert_eq!(r#type, "secret-manager");
                assert!(slug.is_none());
            }
            _ => panic!("wrong subcommand"),
        }

        let cli = parses(&["inf", "identity", "client-secret", "ci"]);
        match cli.command {
            Commands::Identity(IdentityCommands::ClientSecret {
                description,
                ttl,
                num_uses,
                ..
            }) => {
                assert_eq!(description, "created by inf");
                assert_eq!(ttl, 0);
                assert_eq!(num_uses, 0);
            }
            _ => panic!("wrong subcommand"),
        }
    }

    #[test]
    fn project_is_required_only_where_documented() {
        // Required: create and set-role. Optional everywhere else, because
        // pre-existing org-level identities must stay manageable.
        assert!(parse(&["inf", "identity", "create", "--name", "ci"]).is_err());
        assert!(parse(&["inf", "identity", "set-role", "ci"]).is_err());

        for args in [
            vec!["inf", "identity", "ls"],
            vec!["inf", "identity", "get", "ci"],
            vec!["inf", "identity", "rename", "ci", "ci2"],
            vec!["inf", "identity", "delete", "ci", "--yes"],
            vec!["inf", "identity", "client-secret", "ci"],
        ] {
            parses(&args);
        }
    }

    #[test]
    fn positional_order_is_query_then_new_name() {
        let cli = parses(&["inf", "identity", "rename", "old", "new", "--project", "p"]);
        match cli.command {
            Commands::Identity(IdentityCommands::Rename {
                query,
                new_name,
                project,
            }) => {
                assert_eq!(query, "old");
                assert_eq!(new_name, "new");
                assert_eq!(project.as_deref(), Some("p"));
            }
            _ => panic!("wrong subcommand"),
        }

        let cli = parses(&["inf", "project", "rename", "old", "New Name"]);
        match cli.command {
            Commands::Project(ProjectCommands::Rename { query, new_name }) => {
                assert_eq!(query, "old");
                assert_eq!(new_name, "New Name");
            }
            _ => panic!("wrong subcommand"),
        }
    }

    #[test]
    fn documented_project_types_are_accepted_and_others_rejected() {
        for kind in [
            "secret-manager",
            "cert-manager",
            "kms",
            "ssh",
            "secret-scanning",
            "pam",
            "ai",
        ] {
            parses(&["inf", "project", "create", "--name", "p", "--type", kind]);
        }
        assert!(parse(&["inf", "project", "create", "--name", "p", "--type", "nope"]).is_err());
    }

    #[test]
    fn short_yes_flag_is_accepted_on_every_destructive_command() {
        parses(&["inf", "project", "delete", "p", "-y"]);
        parses(&["inf", "project", "env", "delete", "p", "dev", "-y"]);
        parses(&["inf", "identity", "delete", "i", "-y"]);
    }
}
