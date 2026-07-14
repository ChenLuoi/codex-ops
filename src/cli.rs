use crate::limits::{LimitCommand, LimitCommandOptions};
use crate::stats::StatCommandOptions;
use clap::error::ErrorKind;
use clap::{Args, CommandFactory, Parser, Subcommand};
use std::env;
use std::path::PathBuf;

const ROOT_USAGE: &str = "codex-ops <command> [options]";
const AUTH_USAGE: &str = "codex-ops auth <command> [options]";
const AUTH_STATUS_USAGE: &str = "codex-ops auth status [options]";
const AUTH_SAVE_USAGE: &str = "codex-ops auth save [options]";
const AUTH_LIST_USAGE: &str = "codex-ops auth list [options]";
const AUTH_SELECT_USAGE: &str = "codex-ops auth select [options]";
const AUTH_REMOVE_USAGE: &str = "codex-ops auth remove [options]";
const DOCTOR_USAGE: &str = "codex-ops doctor [options]";
const LIMIT_USAGE: &str = "codex-ops limit <command> [options]";
const FAST_USAGE: &str = "codex-ops fast <command> [options]";
const FAST_ON_USAGE: &str = "codex-ops fast on [options]";
const FAST_OFF_USAGE: &str = "codex-ops fast off [options]";
const FAST_STATUS_USAGE: &str = "codex-ops fast status [options]";
const FAST_HISTORY_USAGE: &str = "codex-ops fast history [options]";
const FAST_CANDIDATES_USAGE: &str = "codex-ops fast candidates [options]";

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ParsedCli {
    Command(Box<CliCommand>),
    Help(String),
    Version,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum CliCommand {
    Auth(AuthCliCommand),
    Doctor(DoctorCliCommand),
    Stat(StatCliCommand),
    Limit(LimitCliCommand),
    Fast(FastCliCommand),
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum AuthCliCommand {
    Status(AuthStatusCliOptions),
    Save(AuthProfileCliOptions),
    List(AuthProfileCliOptions),
    Select(AuthSelectCliOptions),
    Remove(AuthRemoveCliOptions),
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum DoctorCliCommand {
    Run(DoctorCliOptions),
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct StatCliCommand {
    pub view: Option<String>,
    pub session: Option<String>,
    pub options: StatCommandOptions,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct LimitCliCommand {
    pub command: LimitCommand,
    pub options: LimitCommandOptions,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum FastCliCommand {
    On(FastCliOptions),
    Off(FastCliOptions),
    Status(FastCliOptions),
    History(FastCliOptions),
    Candidates(Box<StatCommandOptions>),
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct FastCliOptions {
    pub usage_mode_history_file: Option<PathBuf>,
    pub at: Option<String>,
    pub json: bool,
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct AuthCliPaths {
    pub auth_file: Option<PathBuf>,
    pub codex_home: Option<PathBuf>,
    pub store_dir: Option<PathBuf>,
    pub account_history_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AuthStatusCliOptions {
    pub paths: AuthCliPaths,
    pub json: bool,
    pub include_token_claims: bool,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AuthProfileCliOptions {
    pub paths: AuthCliPaths,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AuthSelectCliOptions {
    pub paths: AuthCliPaths,
    pub account_id: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AuthRemoveCliOptions {
    pub paths: AuthCliPaths,
    pub account_id: Option<String>,
    pub yes: bool,
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct DoctorCliPaths {
    pub auth_file: Option<PathBuf>,
    pub codex_home: Option<PathBuf>,
    pub sessions_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DoctorCliOptions {
    pub paths: DoctorCliPaths,
    pub json: bool,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CliParseError {
    pub code: i32,
    pub message: String,
}

impl CliParseError {
    fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "codex-ops",
    disable_help_subcommand = true,
    disable_version_flag = true,
    override_usage = ROOT_USAGE,
    color = clap::ColorChoice::Never
)]
struct CliArgs {
    #[arg(short = 'V', long, help = "Print version")]
    version: bool,

    #[command(subcommand)]
    command: Option<RootCommand>,
}

#[derive(Debug, Subcommand)]
enum RootCommand {
    #[command(
        about = "Show and manage Codex authentication information",
        override_usage = AUTH_USAGE
    )]
    Auth(AuthArgs),
    #[command(
        about = "Check local Codex Ops configuration and data",
        override_usage = DOCTOR_USAGE
    )]
    Doctor(DoctorArgs),
    #[command(
        about = "Show Codex session token usage statistics",
        override_usage = "codex-ops stat [view] [session] [options]"
    )]
    Stat(StatArgs),
    #[command(
        about = "Show Codex server rate-limit telemetry",
        override_usage = LIMIT_USAGE
    )]
    Limit(LimitArgs),
    #[command(
        about = "Record local fast attribution and inspect fast candidates",
        override_usage = FAST_USAGE
    )]
    Fast(FastArgs),
}

#[derive(Debug, Args)]
struct AuthArgs {
    #[command(subcommand)]
    command: Option<AuthCommand>,
}

#[derive(Debug, Subcommand)]
enum AuthCommand {
    #[command(
        about = "Decode auth.json and show key claims",
        override_usage = AUTH_STATUS_USAGE
    )]
    Status(AuthStatusArgs),
    #[command(
        about = "Persist the current auth.json by account id",
        override_usage = AUTH_SAVE_USAGE
    )]
    Save(AuthProfileArgs),
    #[command(
        about = "List current and persisted auth profiles",
        override_usage = AUTH_LIST_USAGE
    )]
    List(AuthProfileArgs),
    #[command(
        about = "Activate a persisted auth profile",
        override_usage = AUTH_SELECT_USAGE
    )]
    Select(AuthSelectArgs),
    #[command(
        about = "Remove persisted auth profiles",
        override_usage = AUTH_REMOVE_USAGE
    )]
    Remove(AuthRemoveArgs),
}

#[derive(Debug, Args)]
struct AuthStatusArgs {
    #[arg(long, value_name = "path", help = "Path to auth.json")]
    auth_file: Option<PathBuf>,
    #[arg(long, value_name = "path", help = "Codex home directory")]
    codex_home: Option<PathBuf>,
    #[arg(short = 'j', long, help = "Print JSON")]
    json: bool,
    #[arg(long, help = "Include decoded JWT header and claims in JSON output")]
    include_token_claims: bool,
}

#[derive(Debug, Args)]
struct AuthProfileArgs {
    #[arg(long, value_name = "path", help = "Path to auth.json")]
    auth_file: Option<PathBuf>,
    #[arg(long, value_name = "path", help = "Codex home directory")]
    codex_home: Option<PathBuf>,
    #[arg(long, value_name = "path", help = "Auth profile store directory")]
    store_dir: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct AuthSelectArgs {
    #[arg(long, value_name = "path", help = "Path to auth.json")]
    auth_file: Option<PathBuf>,
    #[arg(long, value_name = "path", help = "Codex home directory")]
    codex_home: Option<PathBuf>,
    #[arg(long, value_name = "path", help = "Auth profile store directory")]
    store_dir: Option<PathBuf>,
    #[arg(long, value_name = "path", help = "Auth account history file")]
    account_history_file: Option<PathBuf>,
    #[arg(
        short = 'A',
        long,
        value_name = "id",
        help = "Activate a specific persisted account id"
    )]
    account_id: Option<String>,
}

#[derive(Debug, Args)]
struct AuthRemoveArgs {
    #[arg(long, value_name = "path", help = "Path to auth.json")]
    auth_file: Option<PathBuf>,
    #[arg(long, value_name = "path", help = "Codex home directory")]
    codex_home: Option<PathBuf>,
    #[arg(long, value_name = "path", help = "Auth profile store directory")]
    store_dir: Option<PathBuf>,
    #[arg(
        short = 'A',
        long,
        value_name = "id",
        help = "Remove a specific persisted account id"
    )]
    account_id: Option<String>,
    #[arg(
        short = 'y',
        long,
        help = "Skip confirmation when --account-id is supplied"
    )]
    yes: bool,
}

#[derive(Debug, Args)]
struct DoctorArgs {
    #[arg(long, value_name = "path", help = "Path to auth.json")]
    auth_file: Option<PathBuf>,
    #[arg(long, value_name = "path", help = "Codex home directory")]
    codex_home: Option<PathBuf>,
    #[arg(long, value_name = "path", help = "Codex sessions directory")]
    sessions_dir: Option<PathBuf>,
    #[arg(short = 'j', long, help = "Print JSON")]
    json: bool,
}

#[derive(Debug, Args)]
struct StatArgs {
    #[arg(value_name = "view", help = "Optional view: sessions")]
    view: Option<String>,
    #[arg(value_name = "session")]
    session: Option<String>,
    #[command(flatten)]
    options: StatOptionArgs,
}

#[derive(Debug, Args)]
struct StatOptionArgs {
    #[arg(
        short = 'g',
        long,
        value_name = "group",
        help = "hour, day, week, month, model, cwd, account"
    )]
    group_by: Option<String>,
    #[arg(
        long,
        value_name = "window",
        help = "Aggregate usage by server rate-limit windows: 5h or 7d"
    )]
    limit_window: Option<String>,
    #[arg(
        short = 'S',
        long,
        value_name = "sort",
        help = "time, tokens, credits, calls, sessions"
    )]
    sort: Option<String>,
    #[arg(short = 'n', long, value_name = "n", help = "Maximum rows to show")]
    limit: Option<String>,
    #[arg(
        short = 'T',
        long,
        value_name = "n",
        help = "Number of sessions to show"
    )]
    top: Option<String>,
    #[arg(short = 'd', long, help = "Show full event-level rows")]
    detail: bool,
    #[arg(
        short = 'F',
        long,
        help = "Compatibility option; default scanning already checks historical session files"
    )]
    full_scan: bool,
    #[arg(short = 'a', long, help = "Include all session usage")]
    all: bool,
    #[arg(short = 'r', long, help = "Include reasoning effort in model grouping")]
    reasoning_effort: bool,
    #[arg(
        short = 'A',
        long,
        value_name = "id",
        help = "Only include one account id"
    )]
    account_id: Option<String>,
    #[arg(long, value_name = "path", help = "Path to auth.json")]
    auth_file: Option<PathBuf>,
    #[arg(long, value_name = "path", help = "Auth account history file")]
    account_history_file: Option<PathBuf>,
    #[arg(long, value_name = "path", help = "Usage mode history file")]
    usage_mode_history_file: Option<PathBuf>,
    #[arg(long, value_name = "path", help = "Codex home directory")]
    codex_home: Option<PathBuf>,
    #[arg(long, value_name = "path", help = "Codex sessions directory")]
    sessions_dir: Option<PathBuf>,
    #[arg(short = 's', long, value_name = "time", help = "Start time")]
    start: Option<String>,
    #[arg(short = 'e', long, value_name = "time", help = "End time")]
    end: Option<String>,
    #[arg(short = 't', long, help = "Use today as the range")]
    today: bool,
    #[arg(long, help = "Use yesterday as the range")]
    yesterday: bool,
    #[arg(short = 'm', long, help = "Use the current calendar month")]
    month: bool,
    #[arg(
        short = 'L',
        long,
        value_name = "duration",
        help = "Recent duration such as 12h, 7d, 2w, 1mo"
    )]
    last: Option<String>,
    #[arg(
        short = 'f',
        long,
        value_name = "format",
        help = "table, json, csv, markdown"
    )]
    format: Option<String>,
    #[arg(short = 'j', long, help = "Print JSON")]
    json: bool,
    #[arg(short = 'v', long, help = "Show diagnostics")]
    verbose: bool,
}

#[derive(Debug, Args)]
struct LimitArgs {
    #[command(subcommand)]
    command: Option<LimitSubcommand>,
}

#[derive(Debug, Args)]
struct FastArgs {
    #[command(subcommand)]
    command: Option<FastSubcommand>,
}

#[derive(Debug, Subcommand)]
enum FastSubcommand {
    #[command(
        about = "Record local fast attribution as on",
        override_usage = FAST_ON_USAGE
    )]
    On(FastWriteArgs),
    #[command(
        about = "Record local fast attribution as off",
        override_usage = FAST_OFF_USAGE
    )]
    Off(FastWriteArgs),
    #[command(
        about = "Show current local fast attribution",
        override_usage = FAST_STATUS_USAGE
    )]
    Status(FastReadArgs),
    #[command(
        about = "List local fast attribution history",
        override_usage = FAST_HISTORY_USAGE
    )]
    History(FastReadArgs),
    #[command(
        about = "Detect possible fast attribution periods",
        override_usage = FAST_CANDIDATES_USAGE,
        alias = "candidate"
    )]
    Candidates(FastCandidatesArgs),
}

#[derive(Debug, Args)]
struct FastWriteArgs {
    #[arg(long, value_name = "time", help = "Switch timestamp")]
    at: Option<String>,
    #[command(flatten)]
    common: FastCommonArgs,
}

#[derive(Debug, Args)]
struct FastReadArgs {
    #[command(flatten)]
    common: FastCommonArgs,
}

#[derive(Debug, Args)]
struct FastCommonArgs {
    #[arg(long, value_name = "path", help = "Usage mode history file")]
    usage_mode_history_file: Option<PathBuf>,
    #[arg(short = 'j', long, help = "Print JSON")]
    json: bool,
}

#[derive(Debug, Args)]
struct FastCandidatesArgs {
    #[arg(short = 's', long, value_name = "time", help = "Start time")]
    start: Option<String>,
    #[arg(short = 'e', long, value_name = "time", help = "End time")]
    end: Option<String>,
    #[arg(short = 'a', long, help = "Include all session usage")]
    all: bool,
    #[arg(short = 't', long, help = "Use today as the range")]
    today: bool,
    #[arg(long, help = "Use yesterday as the range")]
    yesterday: bool,
    #[arg(short = 'm', long, help = "Use the current calendar month")]
    month: bool,
    #[arg(
        short = 'L',
        long,
        value_name = "duration",
        help = "Recent duration such as 12h, 7d, 2w, 1mo"
    )]
    last: Option<String>,
    #[arg(
        short = 'F',
        long,
        help = "Compatibility option; default scanning already checks historical session files"
    )]
    full_scan: bool,
    #[arg(
        short = 'A',
        long,
        value_name = "id",
        help = "Only include one account id"
    )]
    account_id: Option<String>,
    #[arg(long, value_name = "path", help = "Path to auth.json")]
    auth_file: Option<PathBuf>,
    #[arg(long, value_name = "path", help = "Auth account history file")]
    account_history_file: Option<PathBuf>,
    #[arg(long, value_name = "path", help = "Codex home directory")]
    codex_home: Option<PathBuf>,
    #[arg(long, value_name = "path", help = "Codex sessions directory")]
    sessions_dir: Option<PathBuf>,
    #[arg(
        short = 'f',
        long,
        value_name = "format",
        help = "table, json, csv, markdown"
    )]
    format: Option<String>,
    #[arg(short = 'j', long, help = "Print JSON")]
    json: bool,
    #[arg(short = 'v', long, help = "Show diagnostics")]
    verbose: bool,
}

#[derive(Debug, Subcommand)]
enum LimitSubcommand {
    #[command(
        about = "Show latest observed rate-limit state",
        override_usage = "codex-ops limit current [options]"
    )]
    Current(LimitCurrentArgs),
    #[command(
        about = "List observed rate-limit windows",
        override_usage = "codex-ops limit windows [options]"
    )]
    Windows(LimitCommonArgs),
    #[command(
        about = "Show rate-limit used-percent change timeline",
        override_usage = "codex-ops limit trend [options]"
    )]
    Trend(LimitCommonArgs),
    #[command(
        about = "Show detected rate-limit reset events",
        override_usage = "codex-ops limit resets [options]"
    )]
    Resets(LimitResetArgs),
    #[command(
        about = "Export raw rate-limit samples",
        override_usage = "codex-ops limit samples [options]"
    )]
    Samples(LimitCommonArgs),
}

#[derive(Debug, Args)]
struct LimitResetArgs {
    #[command(flatten)]
    common: LimitCommonArgs,
    #[arg(long, help = "Only include resets before the prior reset time")]
    early_only: bool,
}

#[derive(Debug, Args)]
struct LimitCurrentArgs {
    #[arg(long, value_name = "window", help = "5h or 7d")]
    window: Option<String>,
    #[arg(
        short = 'A',
        long,
        value_name = "id",
        help = "Only include one account id"
    )]
    account_id: Option<String>,
    #[arg(long, value_name = "path", help = "Path to auth.json")]
    auth_file: Option<PathBuf>,
    #[arg(long, value_name = "path", help = "Auth account history file")]
    account_history_file: Option<PathBuf>,
    #[arg(long, value_name = "path", help = "Codex home directory")]
    codex_home: Option<PathBuf>,
    #[arg(long, value_name = "path", help = "Codex sessions directory")]
    sessions_dir: Option<PathBuf>,
    #[arg(
        short = 'f',
        long,
        value_name = "format",
        help = "table, json, csv, markdown"
    )]
    format: Option<String>,
    #[arg(short = 'j', long, help = "Print JSON")]
    json: bool,
    #[arg(short = 'v', long, help = "Show diagnostics")]
    verbose: bool,
}

#[derive(Debug, Args)]
struct LimitCommonArgs {
    #[arg(long, value_name = "window", help = "5h or 7d")]
    window: Option<String>,
    #[arg(
        short = 'A',
        long,
        value_name = "id",
        help = "Only include one account id"
    )]
    account_id: Option<String>,
    #[arg(long, value_name = "path", help = "Path to auth.json")]
    auth_file: Option<PathBuf>,
    #[arg(long, value_name = "path", help = "Auth account history file")]
    account_history_file: Option<PathBuf>,
    #[arg(long, value_name = "path", help = "Codex home directory")]
    codex_home: Option<PathBuf>,
    #[arg(long, value_name = "path", help = "Codex sessions directory")]
    sessions_dir: Option<PathBuf>,
    #[arg(short = 's', long, value_name = "time", help = "Start time")]
    start: Option<String>,
    #[arg(short = 'e', long, value_name = "time", help = "End time")]
    end: Option<String>,
    #[arg(
        short = 'L',
        long,
        value_name = "duration",
        help = "Recent duration such as 12h, 30d, 2w, 1mo"
    )]
    last: Option<String>,
    #[arg(
        short = 'f',
        long,
        value_name = "format",
        help = "table, json, csv, markdown"
    )]
    format: Option<String>,
    #[arg(short = 'j', long, help = "Print JSON")]
    json: bool,
    #[arg(short = 'v', long, help = "Show diagnostics")]
    verbose: bool,
}

pub fn parse_cli(args: &[String]) -> Result<ParsedCli, CliParseError> {
    let argv = std::iter::once("codex-ops".to_string()).chain(args.iter().cloned());
    match CliArgs::try_parse_from(argv) {
        Ok(parsed) => parsed.into_parsed_cli(),
        Err(error) => match error.kind() {
            ErrorKind::DisplayHelp => Ok(ParsedCli::Help(error.to_string())),
            ErrorKind::DisplayVersion => Ok(ParsedCli::Version),
            _ => Err(CliParseError::new(error.exit_code(), error.to_string())),
        },
    }
}

impl CliArgs {
    fn into_parsed_cli(self) -> Result<ParsedCli, CliParseError> {
        if self.version {
            return Ok(ParsedCli::Version);
        }

        match self.command {
            None => Ok(ParsedCli::Help(root_help())),
            Some(RootCommand::Auth(args)) => auth_command(args),
            Some(RootCommand::Doctor(args)) => Ok(parsed_command(CliCommand::Doctor(
                DoctorCliCommand::Run(doctor_options(args)?),
            ))),
            Some(RootCommand::Stat(args)) => {
                Ok(parsed_command(CliCommand::Stat(stat_command(args)?)))
            }
            Some(RootCommand::Limit(args)) => limit_command(args),
            Some(RootCommand::Fast(args)) => fast_command(args),
        }
    }
}

fn auth_command(args: AuthArgs) -> Result<ParsedCli, CliParseError> {
    let command = match args.command {
        None => return Ok(ParsedCli::Help(auth_help())),
        Some(AuthCommand::Status(args)) => AuthCliCommand::Status(AuthStatusCliOptions {
            paths: AuthCliPaths {
                auth_file: resolve_cli_path(args.auth_file)?,
                codex_home: args.codex_home,
                ..AuthCliPaths::default()
            },
            json: args.json,
            include_token_claims: args.include_token_claims,
        }),
        Some(AuthCommand::Save(args)) => AuthCliCommand::Save(AuthProfileCliOptions {
            paths: auth_profile_paths(args)?,
        }),
        Some(AuthCommand::List(args)) => AuthCliCommand::List(AuthProfileCliOptions {
            paths: auth_profile_paths(args)?,
        }),
        Some(AuthCommand::Select(args)) => AuthCliCommand::Select(AuthSelectCliOptions {
            paths: AuthCliPaths {
                auth_file: resolve_cli_path(args.auth_file)?,
                codex_home: args.codex_home,
                store_dir: resolve_cli_path(args.store_dir)?,
                account_history_file: resolve_cli_path(args.account_history_file)?,
            },
            account_id: args.account_id,
        }),
        Some(AuthCommand::Remove(args)) => AuthCliCommand::Remove(AuthRemoveCliOptions {
            paths: AuthCliPaths {
                auth_file: resolve_cli_path(args.auth_file)?,
                codex_home: args.codex_home,
                store_dir: resolve_cli_path(args.store_dir)?,
                account_history_file: None,
            },
            account_id: args.account_id,
            yes: args.yes,
        }),
    };

    Ok(parsed_command(CliCommand::Auth(command)))
}

fn auth_profile_paths(args: AuthProfileArgs) -> Result<AuthCliPaths, CliParseError> {
    Ok(AuthCliPaths {
        auth_file: resolve_cli_path(args.auth_file)?,
        codex_home: args.codex_home,
        store_dir: resolve_cli_path(args.store_dir)?,
        account_history_file: None,
    })
}

fn doctor_options(args: DoctorArgs) -> Result<DoctorCliOptions, CliParseError> {
    Ok(DoctorCliOptions {
        paths: DoctorCliPaths {
            auth_file: resolve_cli_path(args.auth_file)?,
            codex_home: args.codex_home,
            sessions_dir: args.sessions_dir,
        },
        json: args.json,
    })
}

fn stat_command(args: StatArgs) -> Result<StatCliCommand, CliParseError> {
    Ok(StatCliCommand {
        view: args.view,
        session: args.session,
        options: stat_options(args.options)?,
    })
}

fn stat_options(args: StatOptionArgs) -> Result<StatCommandOptions, CliParseError> {
    Ok(StatCommandOptions {
        start: args.start,
        end: args.end,
        group_by: args.group_by,
        limit_window: args.limit_window,
        format: args.format,
        codex_home: args.codex_home,
        sessions_dir: args.sessions_dir,
        auth_file: resolve_cli_path(args.auth_file)?,
        account_history_file: resolve_cli_path(args.account_history_file)?,
        usage_mode_history_file: resolve_cli_path(args.usage_mode_history_file)?,
        today: args.today,
        yesterday: args.yesterday,
        month: args.month,
        all: args.all,
        reasoning_effort: args.reasoning_effort,
        account_id: args.account_id,
        last: args.last,
        sort: args.sort,
        limit: args.limit,
        top: args.top,
        detail: args.detail,
        full_scan: args.full_scan,
        verbose: args.verbose,
        json: args.json,
    })
}

fn limit_command(args: LimitArgs) -> Result<ParsedCli, CliParseError> {
    let command = match args.command {
        None => return Ok(ParsedCli::Help(limit_help())),
        Some(LimitSubcommand::Current(args)) => LimitCliCommand {
            command: LimitCommand::Current,
            options: limit_current_options(args)?,
        },
        Some(LimitSubcommand::Windows(args)) => LimitCliCommand {
            command: LimitCommand::Windows,
            options: limit_options(args, None)?,
        },
        Some(LimitSubcommand::Trend(args)) => LimitCliCommand {
            command: LimitCommand::Trend,
            options: limit_options(args, None)?,
        },
        Some(LimitSubcommand::Resets(args)) => LimitCliCommand {
            command: LimitCommand::Resets,
            options: limit_options(args.common, Some(args.early_only))?,
        },
        Some(LimitSubcommand::Samples(args)) => LimitCliCommand {
            command: LimitCommand::Samples,
            options: limit_options(args, None)?,
        },
    };

    Ok(parsed_command(CliCommand::Limit(command)))
}

fn fast_command(args: FastArgs) -> Result<ParsedCli, CliParseError> {
    let command = match args.command {
        None => return Ok(ParsedCli::Help(fast_help())),
        Some(FastSubcommand::On(args)) => FastCliCommand::On(fast_write_options(args)?),
        Some(FastSubcommand::Off(args)) => FastCliCommand::Off(fast_write_options(args)?),
        Some(FastSubcommand::Status(args)) => FastCliCommand::Status(fast_read_options(args)?),
        Some(FastSubcommand::History(args)) => FastCliCommand::History(fast_read_options(args)?),
        Some(FastSubcommand::Candidates(args)) => {
            FastCliCommand::Candidates(Box::new(fast_candidates_options(args)?))
        }
    };

    Ok(parsed_command(CliCommand::Fast(command)))
}

fn fast_write_options(args: FastWriteArgs) -> Result<FastCliOptions, CliParseError> {
    Ok(FastCliOptions {
        usage_mode_history_file: resolve_cli_path(args.common.usage_mode_history_file)?,
        at: args.at,
        json: args.common.json,
    })
}

fn fast_read_options(args: FastReadArgs) -> Result<FastCliOptions, CliParseError> {
    Ok(FastCliOptions {
        usage_mode_history_file: resolve_cli_path(args.common.usage_mode_history_file)?,
        at: None,
        json: args.common.json,
    })
}

fn fast_candidates_options(args: FastCandidatesArgs) -> Result<StatCommandOptions, CliParseError> {
    Ok(StatCommandOptions {
        start: args.start,
        end: args.end,
        group_by: None,
        limit_window: None,
        format: args.format,
        codex_home: args.codex_home,
        sessions_dir: args.sessions_dir,
        auth_file: resolve_cli_path(args.auth_file)?,
        account_history_file: resolve_cli_path(args.account_history_file)?,
        usage_mode_history_file: None,
        today: args.today,
        yesterday: args.yesterday,
        month: args.month,
        all: args.all,
        reasoning_effort: false,
        account_id: args.account_id,
        last: args.last,
        sort: None,
        limit: None,
        top: None,
        detail: false,
        full_scan: args.full_scan,
        verbose: args.verbose,
        json: args.json,
    })
}

fn limit_current_options(args: LimitCurrentArgs) -> Result<LimitCommandOptions, CliParseError> {
    Ok(LimitCommandOptions {
        start: None,
        end: None,
        last: None,
        format: args.format,
        codex_home: args.codex_home,
        sessions_dir: args.sessions_dir,
        auth_file: resolve_cli_path(args.auth_file)?,
        account_history_file: resolve_cli_path(args.account_history_file)?,
        account_id: args.account_id,
        window: args.window,
        early_only: false,
        json: args.json,
        verbose: args.verbose,
    })
}

fn limit_options(
    args: LimitCommonArgs,
    early_only: Option<bool>,
) -> Result<LimitCommandOptions, CliParseError> {
    Ok(LimitCommandOptions {
        start: args.start,
        end: args.end,
        last: args.last,
        format: args.format,
        codex_home: args.codex_home,
        sessions_dir: args.sessions_dir,
        auth_file: resolve_cli_path(args.auth_file)?,
        account_history_file: resolve_cli_path(args.account_history_file)?,
        account_id: args.account_id,
        window: args.window,
        early_only: early_only.unwrap_or(false),
        json: args.json,
        verbose: args.verbose,
    })
}

fn resolve_cli_path(path: Option<PathBuf>) -> Result<Option<PathBuf>, CliParseError> {
    match path {
        None => Ok(None),
        Some(path) if path.is_absolute() => Ok(Some(path)),
        Some(path) => env::current_dir()
            .map(|cwd| Some(cwd.join(path)))
            .map_err(|error| CliParseError::new(1, error.to_string())),
    }
}

fn parsed_command(command: CliCommand) -> ParsedCli {
    ParsedCli::Command(Box::new(command))
}

fn root_help() -> String {
    let mut command = CliArgs::command();
    command.render_help().to_string()
}

fn auth_help() -> String {
    let mut command = CliArgs::command();
    let auth = command
        .find_subcommand_mut("auth")
        .expect("auth subcommand is defined");
    auth.render_help().to_string()
}

fn limit_help() -> String {
    let mut command = CliArgs::command();
    let limit = command
        .find_subcommand_mut("limit")
        .expect("limit subcommand is defined");
    limit.render_help().to_string()
}

fn fast_help() -> String {
    let mut command = CliArgs::command();
    let fast = command
        .find_subcommand_mut("fast")
        .expect("fast subcommand is defined");
    fast.render_help().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_path_options_from_space_separated_flags() {
        let args = vec![
            "auth".to_string(),
            "status".to_string(),
            "--auth-file".to_string(),
            "fixtures/auth.json".to_string(),
        ];

        let parsed = parse_cli(&args).expect("parse cli");
        let ParsedCli::Command(command) = parsed else {
            panic!("expected command");
        };
        let CliCommand::Auth(AuthCliCommand::Status(options)) = *command else {
            panic!("expected auth status");
        };

        assert_eq!(
            options.paths.auth_file,
            Some(env::current_dir().expect("cwd").join("fixtures/auth.json"))
        );
    }

    #[test]
    fn resolves_path_options_from_equals_flags() {
        let args = vec![
            "fast".to_string(),
            "status".to_string(),
            "--usage-mode-history-file=fixtures/mode-history.json".to_string(),
        ];

        let parsed = parse_cli(&args).expect("parse cli");
        let ParsedCli::Command(command) = parsed else {
            panic!("expected command");
        };
        let CliCommand::Fast(FastCliCommand::Status(options)) = *command else {
            panic!("expected fast status");
        };
        let cwd = env::current_dir().expect("cwd");

        assert_eq!(
            options.usage_mode_history_file,
            Some(cwd.join("fixtures/mode-history.json"))
        );
    }
}
