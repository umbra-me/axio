//! Finding, storing and reporting credentials.
//!
//! The environment always wins over the store, and a provider nobody has heard
//! of is diagnosed as a typo rather than as a missing credential.

use super::*;

/// Find a credential, environment first, then the store.
pub(crate) fn credential(provider: &str) -> Result<(Secret, auth::Source), String> {
    // Before anything about credentials: does this provider exist? Otherwise a
    // typo is diagnosed as a missing credential, the advice is to store one,
    // storing it succeeds, and only the next run says the name was never valid.
    if !auth::is_known(provider) {
        return Err(unknown_provider(provider));
    }

    let env: Vec<(String, String)> = std::env::vars().collect();
    let home = axio_home();
    if let Some(found) = auth::resolve(provider, &home, &env) {
        return Ok(found);
    }

    // Before explaining how to configure this provider, check whether another
    // one is already configured. "You have no credential" is unhelpful when
    // the real situation is "you have one, for something else" — which is what
    // happens to anyone whose only provider is not the default.
    let others: Vec<String> = auth::status(auth::PROVIDERS, &home, &env)
        .into_iter()
        .filter(|(name, source)| name != provider && source.is_some())
        .map(|(name, _)| name)
        .collect();

    let mut message = format!("no credential for `{provider}`.\n\n");

    if let Some(other) = others.first() {
        message.push_str(&format!(
            "`{other}` is configured, but `{provider}` is the one selected.\n\n\
             Use it for this command:\n    AXIO_PROVIDER={other} axio ...\n\n\
             Or make it the default:\n    [model]\n    provider = \"{other}\"\n\
             in {}\n\n",
            config_file_path().display()
        ));
    }

    message.push_str(&format!(
        "Store a credential for `{provider}`:\n    axio auth login --provider {provider}"
    ));
    // Only when there is a variable to name. Splicing the fallback prose into
    // an `export` line hands the user something that cannot be typed.
    if let Some(var) = auth::env_var_for(provider) {
        message.push_str(&format!(
            "\n\nOr set it for this shell:\n    export {var}=..."
        ));
    }
    Err(message)
}

pub(crate) fn unknown_provider(provider: &str) -> String {
    format!(
        "unknown provider `{provider}`; expected one of {}",
        auth::PROVIDERS
            .iter()
            .map(|p| format!("`{p}`"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

pub(crate) fn auth_command(action: &AuthAction) -> u8 {
    let home = axio_home();
    let env: Vec<(String, String)> = std::env::vars().collect();

    match action {
        AuthAction::Login { provider } => {
            // Refuse the name here rather than storing a credential that no run
            // can use and `auth status` cannot even list.
            if !auth::is_known(provider) {
                eprintln!("axio: {}", unknown_provider(provider));
                return 1;
            }
            // Read from stdin, never from an argument. A credential in argv is
            // visible in `ps` to every user on the machine and lands in shell
            // history besides.
            if std::io::stdin().is_terminal() {
                eprintln!(
                    "Paste the credential for `{provider}` and press enter.\n\
                     It will be visible as you type; pipe it in instead if that matters:\n\
                     \n    axio auth login --provider {provider} < key.txt\n"
                );
            }
            let mut input = String::new();
            if std::io::stdin().read_line(&mut input).is_err() {
                eprintln!("axio: could not read the credential");
                return 1;
            }
            let secret = Secret::new(input.trim());
            if secret.is_empty() {
                // Distinguish "you pressed enter" from "there was never a way
                // to type anything". The second happens whenever stdin is
                // /dev/null — a CI step, a task runner, an editor's terminal —
                // and the advice is completely different.
                if input.is_empty() && !std::io::stdin().is_terminal() {
                    eprintln!(
                        "axio: stdin is empty, so there was nothing to read.\n\n\
                         Pipe the credential in:\n    \
                         axio auth login --provider {provider} < key.txt\n\n\
                         Or run this from an interactive terminal to be prompted."
                    );
                } else {
                    eprintln!("axio: no credential given; nothing was stored");
                }
                return 1;
            }

            match auth::save(&home, provider, secret) {
                Ok(path) => {
                    println!(
                        "stored the credential for `{provider}` at {}",
                        path.display()
                    );
                    println!("{}", auth::protection_note());
                    if let Some(var) = auth::env_var_for(provider)
                        && env.iter().any(|(k, v)| k == var && !v.trim().is_empty())
                    {
                        // Otherwise the next run uses the variable and the user
                        // concludes the login did nothing.
                        println!(
                            "note: {var} is set in this shell and takes precedence over the stored credential"
                        );
                    }
                    0
                }
                Err(e) => {
                    eprintln!("axio: could not store the credential: {e}");
                    1
                }
            }
        }

        AuthAction::Status => {
            let rows = auth::status(auth::PROVIDERS, &home, &env);
            for (provider, source) in rows {
                match source {
                    Some(source) => println!("{provider:<18}  {}", source.describe()),
                    None => println!("{provider:<18}  not configured"),
                }
            }
            println!();
            println!("credential file: {}", auth::auth_path(&home).display());
            0
        }

        AuthAction::Logout { provider } => match auth::forget(&home, provider) {
            Ok(true) => {
                println!("removed the stored credential for `{provider}`");
                if let Some(var) = auth::env_var_for(provider)
                    && env.iter().any(|(k, v)| k == var && !v.trim().is_empty())
                {
                    println!("note: {var} is still set in this shell");
                }
                0
            }
            Ok(false) => {
                println!("no stored credential for `{provider}`");
                0
            }
            Err(e) => {
                eprintln!("axio: could not remove the credential: {e}");
                1
            }
        },
    }
}
