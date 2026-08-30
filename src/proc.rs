//! Killing agy means killing everything agy started.
//!
//! agy runs a tool call by shelling out, so the tree under a turn is
//! `agy -> zsh -c '<command>' -> <command>`. Signalling only agy's pid leaves the
//! shell and the command running: they are reparented to PID 1 and run to
//! completion while the host has been told the turn was cancelled. That is worse
//! than not offering cancellation, because it is silent.
//!
//! The obvious fix -- spawn agy in its own process group and signal the group --
//! does not work here, and the reason is worth recording. agy puts each command
//! it runs into a process group of its own. Measured against agy 1.1.22, with the
//! adapter spawning agy as a group leader:
//!
//! ```text
//! PID   PPID  PGID
//! 73606 73578 73606   agy                                  <- leads its own group
//! 73687 73606 73687   zsh -c 'sleep 45 && touch marker'    <- and its own, not agy's
//! 73688 73687 73687   sleep 45
//! ```
//!
//! `killpg(73606)` reaches agy and nothing else. So the tree is killed by walking
//! it: stop agy, snapshot the process table, find every descendant, kill those,
//! then kill agy. The snapshot has to be taken before agy dies, because once it
//! does the kernel reparents its children to PID 1 and the links that identify
//! them are gone. Everything found is `SIGSTOP`ped as it is found, and the table
//! is read again until a read turns up nothing new: stopping agy does not stop
//! the shell agy already started, and that shell can fork its next command
//! between two reads.
//!
//! agy is still spawned into a process group of its own, for one narrow reason:
//! it keeps a signal aimed at the *adapter's* group -- a terminal `Ctrl-C`, a
//! supervisor's `killpg` -- from killing agy before the walk can run. Killing agy
//! first is precisely what erases the parent links, so that accident is not a
//! free extra safety net here, it is a way to lose the tree.
//!
//! On a non-Unix target there is no process table to walk and no signals, so
//! this degrades to killing the direct child, which is what the adapter did
//! before.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use tokio::process::{Child, Command};

/// Builds a command whose child leads a new process group.
///
/// Not a kill mechanism -- see the module docs, `killpg` cannot do this job --
/// but insulation: a signal sent to the adapter's process group must not reach
/// agy, because agy dying is what destroys the parent links this module walks.
///
/// The group is set on a `std::process::Command` and converted, because tokio's
/// own `process_group` is gated behind `tokio_unstable` and 1.38.2 exposes no
/// mutable view of the inner std command. The std method is stable, and this
/// keeps it in safe code -- the alternative, `pre_exec` with `setpgid`, takes on
/// an async-signal-safety obligation for nothing.
pub fn command_in_own_group(program: &str) -> Command {
    let mut std_command = std::process::Command::new(program);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        std_command.process_group(0);
    }
    Command::from(std_command)
}

/// SIGKILLs a child and everything it started.
///
/// On a non-Unix target there is no process table to walk, so only the direct
/// child is killed and anything it started outlives it -- the behaviour this
/// module was written to fix, still in place where it cannot be. Unix also falls
/// back to the direct child when there is no pid to walk from, meaning the child
/// has already exited and been reaped, so there is no tree left.
pub async fn kill_tree(child: &mut Child) {
    if let Some(pid) = child.id() {
        kill_process_tree(pid);
    }
    // Already dead by now on Unix; this goes through tokio so that tokio knows it
    // and can reap the child, and is the whole of the kill everywhere else.
    let _ = child.kill().await;
}

/// Stops `root` and everything below it, then SIGKILLs the lot.
///
/// Stopping first is what makes the snapshot trustworthy, but one stop is not
/// enough. `SIGSTOP` on agy freezes agy; the shell agy already started keeps
/// running and can fork its next command after the table has been read, and that
/// process would then be missing from the list and survive its dead ancestors --
/// the original bug, narrowed. So this scans, stops whatever is new, and scans
/// again until a scan finds nothing new. A stopped process cannot fork, so the
/// set stops growing once the frontier is stopped.
///
/// Deliberately one synchronous function with no `await` in it: a stop that never
/// reached its kill would leave processes suspended for as long as the machine is
/// up, so nothing may be allowed to interleave between the two.
#[cfg(unix)]
fn kill_process_tree(root: u32) {
    let mut stopped: HashSet<u32> = HashSet::from([root]);
    stop(root);
    // A process forking in a loop can outrun any tree killer; this bounds the
    // work rather than pretending otherwise. In the shape this exists for --
    // `zsh -c 'a && b'` -- one rescan is already enough.
    for scan in 0..MAX_SCANS {
        let table = process_table();
        if table.is_empty() {
            if scan == 0 {
                // Not fatal -- `root` is still killed below -- but it silently
                // restores the exact bug this module exists to fix.
                eprintln!(
                    "agy-acp: could not read the process table; agy's own children may survive"
                );
            }
            break;
        }
        let mut found_new = false;
        for pid in descendants(&table, root) {
            if stopped.insert(pid) {
                stop(pid);
                found_new = true;
            }
        }
        if !found_new {
            break;
        }
    }

    // SAFETY: kill is a plain syscall wrapper with no memory arguments. Each pid
    // named a descendant of our own child moments ago; the same pid-reuse window
    // applies here as to any process-tree killer. `root` goes last, so nothing
    // below it is left without the links back to it.
    for pid in stopped.iter().filter(|&&pid| pid != root) {
        unsafe { libc::kill(*pid as libc::pid_t, libc::SIGKILL) };
    }
    unsafe { libc::kill(root as libc::pid_t, libc::SIGKILL) };
}

/// How many times [`kill_process_tree`] will rescan for processes that appeared
/// while it was working.
#[cfg(unix)]
const MAX_SCANS: usize = 8;

/// Suspends one process, so that it cannot start anything else while the tree
/// around it is being read.
#[cfg(unix)]
fn stop(pid: u32) {
    // SAFETY: as in `kill_process_tree`, whose kill this is always paired with.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGSTOP) };
}

/// Every process under `root` in `table`, transitively.
#[cfg(unix)]
fn descendants(table: &[(u32, u32)], root: u32) -> Vec<u32> {
    let mut found: Vec<u32> = Vec::new();
    let mut frontier = vec![root];
    let mut seen: HashSet<u32> = HashSet::from([root]);
    while let Some(parent) = frontier.pop() {
        for &(pid, ppid) in table {
            if ppid == parent && seen.insert(pid) {
                found.push(pid);
                frontier.push(pid);
            }
        }
    }
    found
}

#[cfg(not(unix))]
fn kill_process_tree(_root: u32) {}

/// `(pid, ppid)` for every process on the machine.
///
/// An empty table means the walk finds nothing and only the direct child dies --
/// the old behaviour, not a new failure mode, but also a silent one, so both
/// implementations below prefer the source least likely to be missing.
#[cfg(target_os = "linux")]
fn process_table() -> Vec<(u32, u32)> {
    // `/proc` rather than `ps`: no fork on the kill path, and a slim container
    // image often has no `ps` at all.
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let pid: u32 = name.to_str()?.parse().ok()?;
            let stat = std::fs::read_to_string(entry.path().join("stat")).ok()?;
            Some((pid, ppid_from_proc_stat(&stat)?))
        })
        .collect()
}

/// The parent pid in a `/proc/<pid>/stat` line.
///
/// Not behind a `cfg`, so the parsing is compiled and tested everywhere even
/// though only Linux calls it. The awkward part is field 2, the executable name:
/// it is parenthesised but may itself contain spaces and parentheses, so the
/// fields after it are located from the *last* `)` rather than by splitting the
/// whole line.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn ppid_from_proc_stat(stat: &str) -> Option<u32> {
    let after_comm = &stat[stat.rfind(')')? + 1..];
    let mut fields = after_comm.split_whitespace();
    let _state = fields.next()?;
    fields.next()?.parse().ok()
}

#[cfg(all(unix, not(target_os = "linux")))]
fn process_table() -> Vec<(u32, u32)> {
    // No `/proc` here, so `ps` it is: this runs once per kill, and the two
    // columns are POSIX. Note this forks, briefly blocking the caller's thread.
    // Blocks the calling thread for a fork+exec, a few milliseconds on the kill
    // path. Worth it to keep this callable from anywhere, including the sync
    // shutdown path that has no runtime to defer to.
    let output = match std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid="])
        .output()
    {
        Ok(output) if output.status.success() => output.stdout,
        _ => return Vec::new(),
    };
    String::from_utf8_lossy(&output)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let ppid = fields.next()?.parse().ok()?;
            Some((pid, ppid))
        })
        .collect()
}

/// The agy children running right now, so that shutdown can kill the same trees
/// a cancel would.
#[derive(Clone, Default)]
pub struct LiveChildren {
    inner: Arc<Mutex<HashSet<u32>>>,
}

impl LiveChildren {
    /// Records a child for the lifetime of the returned guard. `None` means the
    /// child had already exited and been reaped, so there is no pid to record.
    pub fn register(&self, pid: Option<u32>) -> ChildGuard {
        if let Some(pid) = pid {
            self.inner.lock().unwrap().insert(pid);
        }
        ChildGuard {
            children: self.clone(),
            pid,
        }
    }

    /// SIGKILLs every registered child and its tree. Safe to call more than once.
    ///
    /// There is no `Child` here to go through tokio with -- shutdown does not own
    /// the handles -- so this leaves the reaping to whoever does, or to PID 1.
    pub fn kill_all(&self) {
        let live: Vec<u32> = self.inner.lock().unwrap().iter().copied().collect();
        for pid in live {
            kill_process_tree(pid);
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }
}

/// Unregisters a child when dropped. Dropping does **not** kill: by the time a
/// turn ends normally the child has been reaped, and killing a reaped pid risks
/// hitting whatever inherited the number.
pub struct ChildGuard {
    children: LiveChildren,
    pid: Option<u32>,
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(pid) = self.pid {
            self.children.inner.lock().unwrap().remove(&pid);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::process::Command;

    /// `true` while the pid names a process that is still running.
    ///
    /// A zombie does not count. `kill(pid, 0)` succeeds for one, and these are
    /// grandchildren this process never reaps -- so on a machine whose PID 1 is
    /// slow to reap, or is a test runner that never does, a killed process would
    /// look alive forever and the tests would fail for the wrong reason.
    fn alive(pid: u32) -> bool {
        // SAFETY: signal 0 performs the permission and existence checks without
        // delivering anything.
        if unsafe { libc::kill(pid as libc::pid_t, 0) != 0 } {
            return false;
        }
        !ps_field(pid, "state=").is_some_and(|state| state.starts_with('Z'))
    }

    /// One `ps` column for one pid, or `None` if it has gone entirely.
    fn ps_field(pid: u32, field: &str) -> Option<String> {
        let output = std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", field])
            .output()
            .ok()?;
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!value.is_empty()).then_some(value)
    }

    /// Reproduces agy's shape: `agy -> zsh -c '<command>' -> <command>`, with the
    /// middle process in a **process group of its own**, which is what agy does
    /// and what makes `killpg` useless here.
    ///
    /// Two levels below the child on purpose. A one-level tree would let a walk
    /// that only looks at direct children pass here and still miss the command
    /// agy is actually running. `set -m` turns on job control, which is what puts
    /// the background subshell in a group of its own -- no external interpreter
    /// needed for it. The `; :` stops that subshell from `exec`ing `sleep` in
    /// place, which is the whole point: it has to be a separate process.
    ///
    /// The detachment is asserted rather than assumed. A shell that ignored
    /// `set -m` would leave the subshell in our group and quietly turn this into
    /// a weaker test than it looks.
    ///
    /// Returns the child, the middle pid, and the deepest pid.
    async fn spawn_detached_tree() -> (Child, u32, u32) {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(r#"set -m; (sleep 30; :) & echo $!; sleep 30"#)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn sh");
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .await
            .expect("read middle pid");
        let middle: u32 = line.trim().parse().expect("middle pid");

        // The deepest process is found the same way the code under test finds
        // anything, which also means the harness fails loudly if the shell
        // optimises the level away rather than quietly testing a shallower tree.
        let deadline = Instant::now() + Duration::from_secs(5);
        let deepest = loop {
            if let Some(&(pid, _)) = process_table().iter().find(|&&(_, ppid)| ppid == middle) {
                break pid;
            }
            assert!(Instant::now() < deadline, "no process below the middle one");
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert!(alive(middle) && alive(deepest), "the tree must be up first");

        let child_group = ps_field(child.id().expect("child pid"), "pgid=");
        let middle_group = ps_field(middle, "pgid=");
        assert!(
            middle_group.is_some() && middle_group != child_group,
            "the middle process must lead a group of its own, as agy's shells do \
             (child {child_group:?}, middle {middle_group:?})"
        );
        (child, middle, deepest)
    }

    /// Polls rather than sleeping a fixed time: the grandchild is reparented to
    /// PID 1 when its shell dies and disappears once PID 1 reaps it, which is
    /// prompt but not synchronous.
    async fn wait_until_gone(pid: u32) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if !alive(pid) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    #[tokio::test]
    async fn kill_tree_reaches_a_grandchild_in_its_own_process_group() {
        let (mut child, middle, deepest) = spawn_detached_tree().await;
        kill_tree(&mut child).await;
        let _ = child.wait().await;
        assert!(
            wait_until_gone(middle).await,
            "the shell agy started must not outlive the kill"
        );
        assert!(
            wait_until_gone(deepest).await,
            "nor must the command running under it -- the walk has to be transitive"
        );
    }

    /// The negative control, and the reason this module walks the tree instead of
    /// calling `killpg`: the grandchild leads its own group, so signalling the
    /// child's group -- or the child alone, as the adapter used to -- misses it.
    #[tokio::test]
    async fn killing_only_the_child_leaves_the_grandchild_running() {
        let (mut child, middle, deepest) = spawn_detached_tree().await;
        let _ = child.kill().await;
        let _ = child.wait().await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        let orphaned = alive(middle) && alive(deepest);
        // SAFETY: cleaning up this test's own processes, which the assert below
        // may be about to fail on.
        unsafe {
            libc::kill(deepest as libc::pid_t, libc::SIGKILL);
            libc::kill(middle as libc::pid_t, libc::SIGKILL);
        }
        assert!(orphaned, "this is the bug being fixed; it should reproduce");
    }

    #[tokio::test]
    async fn kill_all_kills_a_registered_tree() {
        let children = LiveChildren::default();
        let (mut child, middle, deepest) = spawn_detached_tree().await;
        let guard = children.register(child.id());
        children.kill_all();
        let _ = child.wait().await;
        drop(guard);
        assert!(
            wait_until_gone(middle).await && wait_until_gone(deepest).await,
            "shutdown must reach the whole tree, not just agy"
        );
        assert_eq!(children.len(), 0, "the guard unregisters the child");
    }

    #[test]
    fn descendants_walks_transitively_and_stops_at_the_tree() {
        // 1 -> 10 -> 20 -> 30, plus an unrelated 11 -> 21, and a row that
        // claims to be its own parent.
        let table = [(10, 1), (20, 10), (30, 20), (11, 1), (21, 11), (40, 40)];
        let mut found = descendants(&table, 10);
        found.sort_unstable();
        assert_eq!(found, vec![20, 30], "the walk must not stop at direct children");
        assert!(
            descendants(&table, 30).is_empty(),
            "a leaf has no descendants"
        );
        assert!(
            descendants(&table, 40).is_empty(),
            "a self-parenting row must not loop or count itself"
        );
    }

    #[test]
    fn proc_stat_ppid_survives_a_hostile_executable_name() {
        assert_eq!(ppid_from_proc_stat("42 (sleep) S 7 42 42 0 -1"), Some(7));
        // A process can name itself anything, including ") S 1".
        assert_eq!(
            ppid_from_proc_stat("42 (we (ird) S 1) S 7 42 42 0 -1"),
            Some(7)
        );
        assert_eq!(ppid_from_proc_stat("nonsense"), None);
        assert_eq!(ppid_from_proc_stat("42 (sleep) S"), None);
    }

    #[tokio::test]
    async fn the_process_table_contains_this_process() {
        let table = process_table();
        assert!(!table.is_empty(), "ps produced no usable rows");
        let me = std::process::id();
        assert!(
            table.iter().any(|&(pid, _)| pid == me),
            "the snapshot must contain the test process itself"
        );
    }

    #[tokio::test]
    async fn killing_an_already_reaped_child_is_a_no_op() {
        let mut child = Command::new("true").spawn().expect("spawn true");
        let _ = child.wait().await;
        assert_eq!(child.id(), None, "reaped, so there is no pid to walk from");
        kill_tree(&mut child).await;
    }

    #[test]
    fn registering_no_pid_records_nothing() {
        let children = LiveChildren::default();
        let guard = children.register(None);
        assert_eq!(children.len(), 0);
        children.kill_all();
        drop(guard);
    }
}
