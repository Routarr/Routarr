//! The subcommands of `routarr`, run instead of the server: the image's
//! health check, and the ways back for an operator locked out, or facing a
//! database a start refuses.

use crate::config::{self, Config};
use crate::{crypto, db, error, services};

/// What `routarr --help` prints.
pub const USAGE: &str = "Routarr, a routing layer for Radarr and Sonarr.

  routarr                                  start the server
  routarr healthcheck                      ask the server of this configuration whether it answers
  routarr restore <archive>                stage a backup, applied at the next start
  routarr decrypt-backup <archive> <zip>   write an encrypted backup opened
  routarr reset-account [--revoke-keys]    give the account a new password, and with the flag
                                           revoke every application key
  routarr --version                        print the version

The configuration is read from the environment, as the README describes.";

/// Where a refused command line sends its reader.
const SEE_HELP: &str = "`routarr --help` lists the commands";

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
pub enum Invocation {
    Serve,
    Healthcheck,
    ResetAccount { revoke_keys: bool },
    Restore(Option<String>),
    DecryptBackup(Option<String>, Option<String>),
    Help,
    Version,
}

/// Read the command line, the program's name left out. Anything but a known
/// command is refused: started on an argument it does not know, the binary
/// opens the library of the working directory and runs its scheduler against
/// the Arrs configured there.
pub fn invocation(args: &[String]) -> Result<Invocation, String> {
    let (command, rest) = match args.split_first() {
        None => return Ok(Invocation::Serve),
        Some((command, rest)) => (command.as_str(), rest),
    };
    let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
    let unexpected = |argument: &str| format!("unexpected argument '{argument}': {SEE_HELP}");
    let alone = |invocation: Invocation| match rest.first() {
        None => Ok(invocation),
        Some(extra) => Err(unexpected(extra)),
    };
    match command {
        "-h" | "--help" | "help" => alone(Invocation::Help),
        "-V" | "--version" | "version" => alone(Invocation::Version),
        "healthcheck" => alone(Invocation::Healthcheck),
        "reset-account" => match rest.as_slice() {
            [] => Ok(Invocation::ResetAccount { revoke_keys: false }),
            ["--revoke-keys"] => Ok(Invocation::ResetAccount { revoke_keys: true }),
            [extra, ..] => Err(unexpected(extra)),
        },
        "restore" => match rest.as_slice() {
            [] => Ok(Invocation::Restore(None)),
            [archive] => Ok(Invocation::Restore(Some((*archive).to_string()))),
            [_, extra, ..] => Err(unexpected(extra)),
        },
        "decrypt-backup" => match rest.as_slice() {
            [] => Ok(Invocation::DecryptBackup(None, None)),
            [archive] => Ok(Invocation::DecryptBackup(Some((*archive).to_string()), None)),
            [archive, out] => Ok(Invocation::DecryptBackup(
                Some((*archive).to_string()),
                Some((*out).to_string()),
            )),
            [_, _, extra, ..] => Err(unexpected(extra)),
        },
        other => Err(format!("unknown command '{other}': {SEE_HELP}")),
    }
}

/// `routarr healthcheck`, the image's HEALTHCHECK. Only the API's own answer
/// counts: a mount point the probe got wrong reaches the page the interface
/// falls back to, which answers 200 as well.
pub async fn healthcheck(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(4))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let answer: serde_json::Value =
        client.get(config.ping_url()).send().await?.error_for_status()?.json().await?;
    if answer["status"] == "ok" {
        Ok(())
    } else {
        Err(format!("{} answered without the ping's status", config.ping_url()).into())
    }
}

/// `routarr restore <archive>`: stage a backup for the next start with the
/// server stopped, which is the way back when a start refuses the database.
pub async fn restore(
    config: &Config,
    archive: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(archive) = archive else {
        return Err(
            "usage: routarr restore <archive>, a name from the backup folder or a path".into()
        );
    };
    let manifest =
        asking_passphrase(|given| services::backup::stage_offline(config, &archive, given)).await?;
    println!(
        "{archive}, taken by Routarr v{} at schema {}, is restored at the next start.",
        manifest.version, manifest.schema
    );
    if !manifest.includes_master_key {
        println!(
            "It carries no master key: the credentials in it open only with the key in place."
        );
    }
    Ok(())
}

/// `routarr decrypt-backup <archive> <zip>`: write an encrypted archive
/// opened, to read it by hand.
pub async fn decrypt_backup(
    config: &Config,
    archive: Option<String>,
    out: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (Some(archive), Some(out)) = (archive, out) else {
        return Err("usage: routarr decrypt-backup <archive> <zip>, the archive a name from the \
                    backup folder or a path, the zip a path outside it"
            .into());
    };
    let written =
        asking_passphrase(|given| services::backup::decrypt_offline(config, &archive, &out, given))
            .await?;
    println!(
        "{} written. It carries the master key in clear: delete it once read.",
        written.display()
    );
    Ok(())
}

/// Run `attempt` with the archive passphrase from the environment, and again
/// with one typed at the prompt each time it answers that one is needed,
/// while there is a terminal to type it in.
async fn asking_passphrase<T, Attempt, Answer>(
    mut attempt: Attempt,
) -> Result<T, Box<dyn std::error::Error>>
where
    Attempt: FnMut(Option<services::backup::Passphrase>) -> Answer,
    Answer: std::future::Future<Output = error::AppResult<T>>,
{
    let mut given = config::archive_passphrase().map(services::backup::passphrase_of);
    loop {
        match attempt(given.clone()).await {
            Err(error::AppError::PassphraseRequired(refusal))
                if std::io::IsTerminal::is_terminal(&std::io::stdin()) =>
            {
                println!("{refusal}");
                let typed = rpassword::prompt_password("Passphrase: ")?;
                given = Some(services::backup::passphrase_of(typed));
            }
            done => return Ok(done?),
        }
    }
}

/// `routarr reset-account`, for an operator locked out of the `forms` account:
/// `docker exec routarr /app/routarr reset-account`. The image carries no
/// `sqlite3`, and the server can keep running, since a sign-in reads the
/// account each time.
///
/// With `--revoke-keys`, every application key is revoked and a new API key is
/// written, for an operator who believes keys leaked with the password.
pub async fn reset_account(
    config: &Config,
    revoke_keys: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let pool = db::init_pool(config).await?;
    let password = services::accounts::reset_account(&pool, &config.password_path()).await?;
    let revoked =
        if revoke_keys { Some(services::applications::revoke_all(&pool).await?) } else { None };
    pool.close().await;
    println!(
        "The account is reset. Sign in as '{}' with: {password}",
        services::accounts::DEFAULT_USERNAME
    );
    println!(
        "The password is also in {}. Every session was closed.",
        config.password_path().display()
    );
    if let Some(revoked) = revoked {
        println!("Every application key was revoked: {revoked}.");
        if config.api_key.is_some() {
            println!("ROUTARR_API_KEY pins the API key: change the variable and restart.");
        } else {
            let path = config.api_key_path();
            crypto::write_api_key(&path, &crypto::generate_secret()?)?;
            println!(
                "A new API key is in {}. Restart Routarr for it to replace the old one.",
                path.display()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(args: &[&str]) -> Result<Invocation, String> {
        invocation(&args.iter().map(ToString::to_string).collect::<Vec<_>>())
    }

    /// `--version` once started a server on the library of the working
    /// directory, with its scheduler, instead of printing anything.
    #[test]
    fn an_unknown_argument_is_refused_rather_than_starting_the_server() {
        assert_eq!(read(&[]), Ok(Invocation::Serve));
        for refused in [
            &["serve"][..],
            &["--verbose"],
            &["healthcheck", "now"],
            &["reset-account", "--revoke-key"],
            &["restore", "a.zip", "b.zip"],
            &["decrypt-backup", "a.zip", "out.zip", "more"],
        ] {
            let message = read(refused).expect_err(&format!("{refused:?} was accepted"));
            assert!(message.contains("routarr --help"), "{refused:?}: {message}");
        }
    }

    #[test]
    fn help_and_version_answer_on_their_own() {
        for asked in ["--help", "-h", "help"] {
            assert_eq!(read(&[asked]), Ok(Invocation::Help), "{asked}");
        }
        for asked in ["--version", "-V", "version"] {
            assert_eq!(read(&[asked]), Ok(Invocation::Version), "{asked}");
        }
        assert!(read(&["--version", "--help"]).is_err());
    }

    #[test]
    fn each_command_reads_its_own_arguments() {
        assert_eq!(read(&["healthcheck"]), Ok(Invocation::Healthcheck));
        assert_eq!(
            read(&["reset-account", "--revoke-keys"]),
            Ok(Invocation::ResetAccount { revoke_keys: true })
        );
        assert_eq!(read(&["reset-account"]), Ok(Invocation::ResetAccount { revoke_keys: false }));
        assert_eq!(read(&["restore", "a.zip"]), Ok(Invocation::Restore(Some("a.zip".into()))));
        assert_eq!(read(&["restore"]), Ok(Invocation::Restore(None)));
        assert_eq!(
            read(&["decrypt-backup", "a.zip", "out.zip"]),
            Ok(Invocation::DecryptBackup(Some("a.zip".into()), Some("out.zip".into())))
        );
    }
}
