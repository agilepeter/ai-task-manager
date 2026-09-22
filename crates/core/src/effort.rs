//! What the money produced.
//!
//! Spend by work area says where the dollars went. It cannot say whether they
//! bought anything. The one record of output already sitting next to the spend
//! is the git history of that folder, so this pairs the two: 30 days of cost
//! against 30 days of commits in the same directory.
//!
//! It is a ratio, not a verdict. A refactor that deletes a thousand lines is
//! one commit; an afternoon of exploration that ends in nothing is zero. The
//! copy says dollars per commit and stops there, because the app cannot know
//! which of those a number describes, and pretending otherwise would be the
//! kind of fake metric this project exists to avoid.
//!
//! `git` is read, never written: one `log` per area, with a count as output.
//! Folder names stay on this machine, like every other work-area figure.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// Areas to measure, largest spend first. One `git log` each, so this is a
/// budget rather than a limit of the idea.
const MAX_AREAS: usize = 25;
/// Below this much 30-day spend the ratio is noise.
const MIN_COST: f64 = 10.0;

/// One work area's spend against its output.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AreaEffort {
    pub area: String,
    /// 30-day API-equivalent cost for the area.
    pub cost: f64,
    /// Commits touching that directory in the same window. None = not in git.
    pub commits: Option<usize>,
    /// Cost divided by commits. None when there is no history to divide by.
    pub cost_per_commit: Option<f64>,
}

/// Commits touching `dir` in the last `days`, or None if it is not in a git
/// repository. Scoped with `-- .` so an area inside a larger repo counts only
/// its own changes.
pub fn commits_since(dir: &Path, days: u32) -> Option<usize> {
    if !dir.is_dir() {
        return None;
    }
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["log", &format!("--since={days}.days"), "--format=%H", "--", "."])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).lines().filter(|l| !l.trim().is_empty()).count())
}

/// Pair each area's cost with the commits in its folder.
///
/// `areas` is (area name, cost) and `root` is the project directory the names
/// hang off. Areas are counted with `count`, which the tests replace so the
/// arithmetic can be checked without a repository.
pub fn measure_with(
    areas: &[(String, f64)],
    root: &Path,
    mut count: impl FnMut(&Path) -> Option<usize>,
) -> Vec<AreaEffort> {
    let mut ranked: Vec<&(String, f64)> = areas.iter().filter(|(_, c)| *c >= MIN_COST).collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    ranked.truncate(MAX_AREAS);
    ranked
        .into_iter()
        .map(|(area, cost)| {
            // "(unsorted)" and friends are not folders, so they are never probed.
            let commits = (!area.starts_with('(')).then(|| count(&root.join(area))).flatten();
            AreaEffort {
                area: area.clone(),
                cost: *cost + 0.0,
                commits,
                // Zero commits is a real answer, not a divisor.
                cost_per_commit: commits.filter(|c| *c > 0).map(|c| cost / c as f64),
            }
        })
        .collect()
}

/// The same, reading real git history.
pub fn measure(areas: &[(String, f64)], root: &Path, days: u32) -> Vec<AreaEffort> {
    measure_with(areas, root, |dir| commits_since(dir, days))
}

/// Every area of every Claude project, measured against its own folder.
pub fn measure_spend(spend: &[crate::spend::ProviderSpend], days: u32) -> Vec<AreaEffort> {
    let mut out: Vec<AreaEffort> = Vec::new();
    for provider in spend.iter().filter(|p| p.id == "claude") {
        for project in &provider.projects {
            let root = PathBuf::from(&project.project);
            if !root.is_absolute() {
                continue; // a lossy folder name, not a path we can look inside
            }
            let areas: Vec<(String, f64)> =
                project.areas.iter().map(|a| (a.area.clone(), a.last30.cost)).collect();
            out.extend(measure(&areas, &root, days));
        }
    }
    out.sort_by(|a, b| b.cost.total_cmp(&a.cost));
    out.truncate(MAX_AREAS);
    out
}

/// Prints this machine's real figures. Ignored: it runs git.
#[test]
#[ignore]
fn live_effort() {
    let spend = crate::spend::collect(None);
    let rows = measure_spend(&spend, 30);
    println!("{:<28} {:>9} {:>8} {:>12}", "area", "30d cost", "commits", "$ / commit");
    for r in &rows {
        println!(
            "{:<28} {:>9.2} {:>8} {:>12}",
            r.area,
            r.cost,
            r.commits.map(|c| c.to_string()).unwrap_or_else(|| "not git".into()),
            r.cost_per_commit.map(|v| format!("{v:.2}")).unwrap_or_else(|| "-".into()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn areas() -> Vec<(String, f64)> {
        vec![("automations".into(), 458.0), ("staasfund".into(), 227.0), ("tiny".into(), 2.0), ("(unsorted)".into(), 90.0)]
    }

    #[test]
    fn divides_cost_by_commits() {
        let got = measure_with(&areas(), Path::new("/w"), |p| {
            Some(if p.ends_with("automations") { 31 } else { 10 })
        });
        let a = got.iter().find(|r| r.area == "automations").expect("present");
        assert_eq!(a.commits, Some(31));
        assert!((a.cost_per_commit.expect("a ratio") - 458.0 / 31.0).abs() < 1e-9);
    }

    #[test]
    fn a_folder_outside_git_says_so_instead_of_guessing() {
        let got = measure_with(&areas(), Path::new("/w"), |_| None);
        let a = got.iter().find(|r| r.area == "automations").expect("present");
        assert_eq!(a.commits, None);
        assert_eq!(a.cost_per_commit, None);
    }

    #[test]
    fn spend_with_no_commits_is_not_divided_by_zero() {
        let got = measure_with(&areas(), Path::new("/w"), |_| Some(0));
        let a = got.iter().find(|r| r.area == "automations").expect("present");
        assert_eq!(a.commits, Some(0), "zero is a real answer");
        assert_eq!(a.cost_per_commit, None, "and never a divisor");
    }

    #[test]
    fn small_areas_are_left_out() {
        let got = measure_with(&areas(), Path::new("/w"), |_| Some(1));
        assert!(got.iter().all(|r| r.area != "tiny"), "$2 makes no ratio worth reading");
    }

    #[test]
    fn a_bucket_that_is_not_a_folder_is_never_probed() {
        let mut probed: Vec<String> = Vec::new();
        let got = measure_with(&areas(), Path::new("/w"), |p| {
            probed.push(p.display().to_string());
            Some(3)
        });
        assert!(probed.iter().all(|p| !p.contains("unsorted")), "{probed:?}");
        let u = got.iter().find(|r| r.area == "(unsorted)").expect("still listed");
        assert_eq!(u.commits, None);
    }

    #[test]
    fn largest_spend_first() {
        let got = measure_with(&areas(), Path::new("/w"), |_| Some(1));
        assert_eq!(got[0].area, "automations");
        assert_eq!(got[1].area, "staasfund");
        assert_eq!(got[2].area, "(unsorted)");
    }
}
