# Running Taskologic quickly for deving.

Literally just run this :
```bash
scripts/dev.sh up
```

That builds the workspace, writes `dev/taskologicd.toml` and `dev/client.toml` then starts the daemon in the background with debug which go here `dev/taskologicd.log`, it obv. runs the TUI in your terminal, and stops the demon when you quit the TUI. You are the first user to log in, so you are an admin btw.

Other things it can do:

| Command                  | What it do                           |
|--------------------------|--------------------------------------|
| `scripts/dev.sh daemon`  | Demon in the foreground              |
| `scripts/dev.sh client`  | Slayer only                          |
| `scripts/dev.sh stop`    | Stop a background demon              |
| `scripts/dev.sh socat`   | Raw JSON Derulo lines                |
| `scripts/dev.sh sql`     | sqlite3 on the dev db                |
| `scripts/dev.sh barcode` | Print a scannable payload for a task |
| `scripts/dev.sh reset`   | Clear it all mhm                     |

Of note : If you are developing stuff that changes the views check out the other `running.md` file as the tests could fail if you dont update the snapshots