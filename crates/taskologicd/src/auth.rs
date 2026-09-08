//! Who is on the other end of the socket.
//!
//! The kernel tells us the uid via SO_PEERCRED, so there is no handshake and
//! no way to claim to be someone else. Being a Taskologic user means being in
//! the configured system group. Remote clients will need real auth (PAM)
//! later, the protocol leaves room for it but nothing here does it.

use nix::unistd::{Gid, Group, Uid, User};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub uid: u32,
    pub username: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("uid {0} has no passwd entry")]
    UnknownUid(u32),
    #[error("group {0} does not exist on this host")]
    NoSuchGroup(String),
    #[error("{0} is not a member of the {1} group")]
    NotInGroup(String, String),
    #[error("user database lookup failed: {0}")]
    Lookup(#[from] nix::Error),
}

pub fn identify(uid: u32, group: &str) -> Result<Identity, AuthError> {
    let user = User::from_uid(Uid::from_raw(uid))?.ok_or(AuthError::UnknownUid(uid))?;
    let g = Group::from_name(group)?.ok_or_else(|| AuthError::NoSuchGroup(group.into()))?;
    let member = user.gid == g.gid || g.mem.contains(&user.name);
    if !member {
        return Err(AuthError::NotInGroup(user.name, group.into()));
    }
    Ok(Identity {
        uid,
        username: user.name,
    })
}

/// Names of everyone in the group, for assignee pickers on public boards.
/// Users with the group as their primary group are not in `mem`, which is
/// why this also scans the passwd database.
pub fn group_members(group: &str) -> Result<Vec<Identity>, AuthError> {
    let g = Group::from_name(group)?.ok_or_else(|| AuthError::NoSuchGroup(group.into()))?;
    let mut out = Vec::new();
    for name in &g.mem {
        if let Some(u) = User::from_name(name)? {
            out.push(Identity {
                uid: u.uid.as_raw(),
                username: u.name,
            });
        }
    }
    out.extend(primary_group_users(g.gid));
    out.sort_by_key(|i| i.uid);
    out.dedup_by_key(|i| i.uid);
    Ok(out)
}

#[cfg(target_os = "linux")]
fn primary_group_users(gid: Gid) -> Vec<Identity> {
    // getpwent is not thread safe and nix does not wrap it, so read the file
    // instead. NSS backed accounts are a future problem.
    std::fs::read_to_string("/etc/passwd")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            let uid: u32 = f.get(2)?.parse().ok()?;
            let g: u32 = f.get(3)?.parse().ok()?;
            (g == gid.as_raw()).then(|| Identity {
                uid,
                username: f[0].to_string(),
            })
        })
        .collect()
}

#[cfg(not(target_os = "linux"))]
fn primary_group_users(_gid: Gid) -> Vec<Identity> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_user_is_in_their_primary_group() {
        let me = User::from_uid(nix::unistd::getuid()).unwrap().unwrap();
        let g = Group::from_gid(me.gid).unwrap().unwrap();
        let id = identify(me.uid.as_raw(), &g.name).unwrap();
        assert_eq!(id.username, me.name);
        assert!(matches!(
            identify(me.uid.as_raw(), "taskologic-no-such-group-xyz"),
            Err(AuthError::NoSuchGroup(_))
        ));
        assert!(matches!(
            identify(u32::MAX - 7, &g.name),
            Err(AuthError::UnknownUid(_))
        ));
    }
}
