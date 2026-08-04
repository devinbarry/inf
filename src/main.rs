mod client;
mod commands;
mod config;
mod error;
mod models;
mod output;
mod resolve;

use clap::{Parser, Subcommand};
use client::InfisicalClient;
use colored::Colorize;
use config::Config;
use error::Result;

#[derive(Parser)]
#[command(
    name = "inf",
    version,
    about = "Infisical control-plane CLI - projects, environments and machine identities",
    long_about = "inf manages the Infisical resources the official `infisical` CLI cannot: \
                  projects, environments and machine identities.\n\n\
                  It reuses the session established by `infisical login`, so no separate \
                  credentials are needed. Secret read/write stays with the official CLI."
)]
struct Cli {
    /// Infisical instance URL. Defaults to $INFISICAL_DOMAIN, then the instance
    /// `infisical login` last used.
    #[arg(long, global = true)]
    domain: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show the active session: instance, organization and expiry
    Whoami {
        #[arg(long)]
        json: bool,
    },
    /// Manage projects and their environments
    #[command(subcommand)]
    Project(ProjectCommands),
    /// Manage machine identities
    #[command(subcommand, visible_alias = "id")]
    Identity(IdentityCommands),
}

#[derive(Subcommand)]
enum ProjectCommands {
    /// List projects
    #[command(visible_alias = "list")]
    Ls {
        #[arg(long)]
        json: bool,
    },
    /// Show one project and its environments
    Get {
        /// Project id, slug, name, or a unique substring
        query: String,
        #[arg(long)]
        json: bool,
    },
    /// Create a project
    Create {
        /// Project name
        #[arg(long)]
        name: String,
        /// URL-safe slug. Derived from the name when omitted.
        #[arg(long)]
        slug: Option<String>,
        #[arg(long)]
        description: Option<String>,
        /// Product this project belongs to
        #[arg(long, default_value = "secret-manager",
              value_parser = ["secret-manager", "cert-manager", "kms", "ssh", "secret-scanning", "pam", "ai"])]
        r#type: String,
        #[arg(long)]
        json: bool,
    },
    /// Rename a project
    Rename {
        /// Project id, slug, name, or a unique substring
        query: String,
        /// New name
        new_name: String,
        #[arg(long)]
        json: bool,
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
        #[arg(long)]
        json: bool,
    },
    /// Delete a project and every secret in it
    #[command(visible_alias = "rm")]
    Delete {
        /// Project id, slug, name, or a unique substring
        query: String,
        /// Skip the confirmation prompt
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// List machine identities with access to a project
    Identities {
        /// Project id, slug, name, or a unique substring
        query: String,
        #[arg(long)]
        json: bool,
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
        #[arg(long)]
        json: bool,
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
        #[arg(long)]
        json: bool,
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
        #[arg(long)]
        json: bool,
    },
    /// Delete an environment and its secrets
    #[command(visible_alias = "rm")]
    Delete {
        /// Project id, slug, name, or a unique substring
        project: String,
        /// Environment slug, id or name
        env: String,
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
        #[arg(long)]
        json: bool,
    },
    /// Show one identity
    Get {
        /// Identity id, name, or a unique substring
        query: String,
        /// Project the identity is scoped to
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        json: bool,
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
        /// Project role. Defaults to read-only.
        #[arg(long, default_value = "viewer")]
        role: String,
        /// Also attach Universal Auth and mint a client secret, so the
        /// identity is immediately usable
        #[arg(long)]
        with_universal_auth: bool,
        #[arg(long)]
        json: bool,
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
        #[arg(long)]
        json: bool,
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
        #[arg(long)]
        json: bool,
    },
    /// Delete an identity
    #[command(visible_alias = "rm")]
    Delete {
        /// Identity id, name, or a unique substring
        query: String,
        /// Project the identity is scoped to
        #[arg(long)]
        project: Option<String>,
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
        #[arg(long)]
        json: bool,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(err) = run(cli).await {
        eprintln!("{} {err}", "error:".red().bold());
        std::process::exit(err.exit_code());
    }
}

async fn run(cli: Cli) -> Result<()> {
    let config = Config::load(cli.domain.as_deref())?;

    // whoami is answered entirely from the decoded session, so it stays useful
    // when the API is unreachable.
    if let Commands::Whoami { json } = &cli.command {
        return whoami(&config, *json);
    }

    let client = InfisicalClient::new(config);

    match cli.command {
        Commands::Whoami { .. } => unreachable!("handled above"),

        Commands::Project(command) => match command {
            ProjectCommands::Ls { json } => commands::project::list(&client, json).await,
            ProjectCommands::Get { query, json } => {
                commands::project::get(&client, &query, json).await
            }
            ProjectCommands::Create {
                name,
                slug,
                description,
                r#type,
                json,
            } => {
                commands::project::create(
                    &client,
                    &name,
                    slug.as_deref(),
                    description.as_deref(),
                    &r#type,
                    json,
                )
                .await
            }
            ProjectCommands::Rename {
                query,
                new_name,
                json,
            } => commands::project::rename(&client, &query, &new_name, json).await,
            ProjectCommands::Update {
                query,
                name,
                slug,
                description,
                delete_protection,
                no_delete_protection,
                json,
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
                    json,
                )
                .await
            }
            ProjectCommands::Delete { query, yes } => {
                commands::project::delete(&client, &query, yes).await
            }
            ProjectCommands::Identities { query, json } => {
                commands::project::identities(&client, &query, json).await
            }
            ProjectCommands::Env(command) => match command {
                EnvCommands::Ls { project, json } => {
                    commands::project::env_list(&client, &project, json).await
                }
                EnvCommands::Create {
                    project,
                    name,
                    slug,
                    json,
                } => {
                    commands::project::env_create(&client, &project, &name, slug.as_deref(), json)
                        .await
                }
                EnvCommands::Update {
                    project,
                    env,
                    name,
                    slug,
                    json,
                } => {
                    commands::project::env_update(
                        &client,
                        &project,
                        &env,
                        name.as_deref(),
                        slug.as_deref(),
                        json,
                    )
                    .await
                }
                EnvCommands::Delete { project, env, yes } => {
                    commands::project::env_delete(&client, &project, &env, yes).await
                }
            },
        },

        Commands::Identity(command) => match command {
            IdentityCommands::Ls { project, json } => {
                commands::identity::list(&client, project.as_deref(), json).await
            }
            IdentityCommands::Get {
                query,
                project,
                json,
            } => commands::identity::get(&client, &query, project.as_deref(), json).await,
            IdentityCommands::Create {
                name,
                project,
                role,
                with_universal_auth,
                json,
            } => {
                commands::identity::create(
                    &client,
                    &name,
                    &project,
                    &role,
                    with_universal_auth,
                    json,
                )
                .await
            }
            IdentityCommands::Rename {
                query,
                new_name,
                project,
                json,
            } => {
                commands::identity::rename(&client, &query, &new_name, project.as_deref(), json)
                    .await
            }
            IdentityCommands::SetRole {
                query,
                project,
                role,
                json,
            } => commands::identity::set_role(&client, &query, &project, &role, json).await,
            IdentityCommands::Delete {
                query,
                project,
                yes,
            } => commands::identity::delete(&client, &query, project.as_deref(), yes).await,
            IdentityCommands::ClientSecret {
                query,
                project,
                description,
                ttl,
                num_uses,
                json,
            } => {
                commands::identity::client_secret(
                    &client,
                    &query,
                    project.as_deref(),
                    &description,
                    ttl,
                    num_uses,
                    json,
                )
                .await
            }
        },
    }
}

fn whoami(config: &Config, json: bool) -> Result<()> {
    let days = config.days_until_expiry();

    if json {
        output::json(&serde_json::json!({
            "domain": config.api_url,
            "organizationId": config.org_id,
            "expiresAt": config.expires_at,
            "daysUntilExpiry": days,
        }));
        return Ok(());
    }

    println!("{} {}", "Instance:".bold(), config.api_url);
    println!("{} {}", "Organization:".bold(), config.org_id);
    match days {
        Some(days) if days <= 1 => {
            println!("{} {}", "Session:".bold(), "expires within a day".yellow());
            output::warn("run `infisical login` soon; the CLI cannot refresh this session");
        }
        Some(days) => println!("{} valid for {} more days", "Session:".bold(), days),
        None => println!("{} valid", "Session:".bold()),
    }
    Ok(())
}
