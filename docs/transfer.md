# Moving things around: programs, templates, boards, whole servers

`scripts/transfer.sh` writes things to JSON files and reads them back, here
or on another server. It runs the one shot commands of `taskologicd` with
the installed binary and config, or the checkout's own under `dev/` when
there is one, and goes through sudo only when the database is not yours to
write. Every command needs root or a Taskologic admin; sudo passes on who
asked, so `sudo` in front does not make a non admin one. The daemon may
keep running; the panels show what came in when reopened.

```bash
scripts/transfer.sh export program  "Kitchen" "Clean up" cleanup.json
scripts/transfer.sh import program  "Garage"  cleanup.json
scripts/transfer.sh export template "Kitchen" "Wash up"  washup.json
scripts/transfer.sh import template "Garage"  washup.json
scripts/transfer.sh export board    "Kitchen" kitchen.json    # or: all
scripts/transfer.sh import board    kitchen.json
scripts/transfer.sh export all      server.json
scripts/transfer.sh import all      server.json
scripts/transfer.sh users
scripts/transfer.sh users remap     server.json [remapped.json]
```

Without a file an export prints to stdout, and an import reads `-` as
stdin. `scripts/demo/` holds example files to import, starting with the
worked example program "Clean Up Entire Apartment".

**Programs and templates** land on the board you name. A template travels
with the templates it depends on; one the target board already has by that
name is used as it is rather than doubled, so rename it in the file for a
fresh copy. Anyone named in the file who is not on the target board is
dropped on import, and the import says so.

**Short ids travel too.** Boards, templates and programs carry their six
character ids in every file. An import keeps them unless this server has
one already, in which case it refuses and says which board has it; add
`--new-id` to import the arriving ones as new things with fresh ids, or
`--replace` to overwrite the template or program that has the id, in place,
its history staying with it. Boards cannot be replaced, only given new ids.

**A board** travels with everything on it: columns, members, tasks live and
archived, their dependencies and repetitions, templates, programs, runs,
print bookkeeping and the whole history. `export board all` is every board
in one file. On import each board arrives as a new one with fresh ids and
its history intact; a task keeps its short id unless that is taken here,
in which case it gets a new one and its old slips stop scanning. An import
stops before writing anything if a board of that name is here already, so
running it twice cannot double a board: rename or delete the old one, or
change the name in the file.

**A whole server** is `export all`: every board as above plus every
account, admins and all, minus kiosk PINs, which people set again. `import
all` is for a fresh server. Somebody has to be able to run it there: root
can, or log in once first, which makes you the admin.

**People are keyed by uid**, and uids differ between servers. Every file
lists the people it names, uid and username, so `users remap FILE` can
rewrite the file's uids to this server's by username before you import it;
it says who it matched and who it could not, and leaves the rest as they
are. `users` on its own shows who this server knows: everyone who has
logged in, and everyone in the daemon's group who has not. A board import
also makes plain accounts for the people it names, so their names show
before they log in; it skips that on a server nobody has logged in to yet,
so the first login still becomes the admin.
