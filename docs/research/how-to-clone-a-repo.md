# How to clone a git repo?

I know it seems simple but really what we mean is how do we clone a git repo non interactively?

These are examples of how one might do it using bash, obviously we will be writing Rust code to do this but the concept
will follow similarly.

## HTTPs

```sh
GIT_TERMINAL_PROMPT=0 \
GIT_CONFIG_COUNT=1 \
GIT_CONFIG_KEY_0="http.https://provider.com/.extraheader" \
GIT_CONFIG_VALUE_0="Authorization: Basic $(printf '%s:%s' "$USERNAME" "$PASSWORD" | base64 -w0)" \
git clone https://provider.com/path/to/repo.git
```

Where:

- `$USERNAME`: Generally can be any value but some providers do ask for very specific values, hence why we can not just
  ask for a password (or token) alone.

- `$PASSWORD`: Generally these are PATs (eg: GITHUB_TOKEN) or some kind of API Key.

> NB: Keep in mind we can infer the value of GIT_CONFIG_KEY_0 by extracting it from the given origin URL.

## SSH

```sh
GIT_TERMINAL_PROMPT=0 \
GIT_SSH_COMMAND="ssh -i $KEY -o IdentitiesOnly=yes -o BatchMode=yes -o StrictHostKeyChecking=accept-new" \
git clone git@provider.com:path/to/repo.git
```

Where:

- `$KEY`: Refer to the path to an OpenSSH private key file that is not protected by a passphrase.
