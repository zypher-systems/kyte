//! Building a paper's PDF. The build runs beside the editor, so typing carries
//! on, and its output is read for problems to jump to.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const DEFAULT_COMMAND: &str = "latexmk -pdf -interaction=nonstopmode -file-line-error %f";
const OPEN_COMMAND: &str = "xdg-open %f";
/// How long to wait for the last of a finished build's output.
const LAST_OUTPUT: Duration = Duration::from_millis(200);

/// Something the build reported, with where to go to see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// As the build named it, relative to the paper's folder.
    pub file: String,
    /// 0 when the build didn't say.
    pub line: usize,
    pub text: String,
}

impl Problem {
    pub fn label(&self) -> String {
        if self.line == 0 {
            format!("{}  {}", self.file, self.text)
        } else {
            format!("{}:{}  {}", self.file, self.line, self.text)
        }
    }
}

/// How a build ended.
#[derive(Debug, PartialEq, Eq)]
pub struct Outcome {
    pub ok: bool,
    pub errors: Vec<Problem>,
    pub warnings: Vec<Problem>,
    /// The last thing it said, for a failure with no errors to list.
    pub last_line: String,
    pub took: Duration,
}

/// A `file:line: message` line, the form `-file-line-error` gives.
fn error_line(line: &str) -> Option<Problem> {
    let (file, rest) = line.split_once(':')?;
    let (number, text) = rest.split_once(':')?;
    let tex = [".tex", ".ltx", ".sty", ".cls", ".bib", ".bbl"];
    if !tex.iter().any(|t| file.ends_with(t)) {
        return None;
    }
    Some(Problem {
        file: file.strip_prefix("./").unwrap_or(file).to_string(),
        line: number.parse().ok()?,
        text: text.trim().to_string(),
    })
}

/// A warning worth acting on: a citation or label that isn't defined. LaTeX's
/// closing `There were undefined references` adds nothing to those.
fn undefined(line: &str, root: &str) -> Option<Problem> {
    if !line.contains("Warning") || !line.contains("undefined") || !line.contains('`') {
        return None;
    }
    let (_, text) = line.split_once("Warning:")?;
    let text = text.trim();
    let number = text
        .rsplit("input line ")
        .next()
        .and_then(|n| n.trim_end_matches('.').parse().ok())
        .unwrap_or(0);
    let text = text.split(" on input line").next().unwrap_or(text);
    Some(Problem {
        file: root.to_string(),
        line: number,
        text: text.to_string(),
    })
}

/// What's wrong in a build's output, each thing once. A build runs LaTeX several
/// times, so the same line comes back.
pub fn problems(output: &str, root: &str) -> (Vec<Problem>, Vec<Problem>) {
    let (mut errors, mut warnings) = (Vec::new(), Vec::new());
    // A `!` error, such as a missing package, gives its place on a later line.
    let mut placeless: Option<usize> = None;
    let last_pass = output.rfind("This is ").unwrap_or(0);
    for line in output.lines() {
        if let Some(text) = line.strip_prefix("! ") {
            let p = Problem {
                file: root.to_string(),
                line: 0,
                text: text.trim().to_string(),
            };
            if !errors.contains(&p) {
                errors.push(p);
                placeless = Some(errors.len() - 1);
            }
        } else if let Some(p) = error_line(line) {
            let closing = p.text == "Emergency stop." || p.text.starts_with("==>");
            match placeless.take() {
                Some(i) if closing => {
                    errors[i].file = p.file;
                    errors[i].line = p.line;
                }
                _ if closing && !errors.is_empty() => {}
                _ if p.text.starts_with("==>") => {}
                _ => {
                    if !errors.contains(&p) {
                        errors.push(p);
                    }
                }
            }
        } else if let Some(p) = undefined(line, root) {
            // A first pass warns about every citation; later ones settle them.
            let from = line.as_ptr() as usize - output.as_ptr() as usize;
            if from >= last_pass && !warnings.contains(&p) {
                warnings.push(p);
            }
        }
    }
    (errors, warnings)
}

/// The line that says most about a failure with no errors to list. `latexmk`
/// ends with advice, so what it collected comes first.
fn last_word(output: &str) -> String {
    let mut lines = output.lines();
    let summary = lines
        .by_ref()
        .find(|l| l.starts_with("Collected error summary"));
    let said = summary.and_then(|_| lines.next());
    said.or_else(|| output.lines().rev().find(|l| !l.trim().is_empty()))
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// `name` quoted for the shell.
fn quoted(name: &str) -> String {
    format!("'{}'", name.replace('\'', "'\\''"))
}

/// `template` with `%f` as the quoted `name`, or with it added at the end.
fn filled(template: &str, name: &str) -> String {
    if template.contains("%f") {
        template.replace("%f", &quoted(name))
    } else {
        format!("{template} {}", quoted(name))
    }
}

/// Whether `program` is on `path`, a `PATH` value.
fn on_path(program: &str, path: &str) -> bool {
    path.split(':')
        .any(|dir| Path::new(dir).join(program).is_file())
}

/// The command to build `name` with. `None` means there's nothing to run it.
pub fn command_for(custom: Option<&str>, name: &str, path: &str) -> Option<String> {
    match custom {
        Some(c) => Some(filled(c, name)),
        None => on_path("latexmk", path).then(|| filled(DEFAULT_COMMAND, name)),
    }
}

fn shell(command: &str, dir: &Path) -> Command {
    let mut c = Command::new("sh");
    c.args(["-c", command])
        .current_dir(dir)
        .stdin(Stdio::null())
        .process_group(0);
    c
}

/// Hands `pdf` to a viewer. The viewer outlives nothing here: it's left to run,
/// and a thread collects it when it ends.
pub fn open_pdf(custom: Option<&str>, pdf: &Path) -> std::io::Result<()> {
    let name = pdf.file_name().unwrap_or_default().to_string_lossy();
    let dir = pdf.parent().unwrap_or(Path::new("."));
    let command = filled(custom.unwrap_or(OPEN_COMMAND), &name);
    let mut child = shell(&command, dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// A build in progress.
pub struct Build {
    child: Child,
    output: Arc<Mutex<Vec<u8>>>,
    reader: JoinHandle<()>,
    started: Instant,
    /// The paper being built, as a name in `dir`.
    pub name: String,
    pub dir: PathBuf,
}

impl Build {
    /// Starts `command` in `dir`.
    pub fn start(command: &str, dir: &Path, name: &str) -> std::io::Result<Build> {
        let mut child = shell(&format!("{{ {command}\n}} 2>&1"), dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            // TeX wraps its output at 79 columns, which splits file names.
            .env("max_print_line", "1000")
            .spawn()?;
        let mut pipe = child.stdout.take().expect("piped");
        let output = Arc::new(Mutex::new(Vec::new()));
        let shared = output.clone();
        let reader = std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            while let Ok(n) = pipe.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                shared.lock().unwrap().extend_from_slice(&chunk[..n]);
            }
        });
        Ok(Build {
            child,
            output,
            reader,
            started: Instant::now(),
            name: name.to_string(),
            dir: dir.to_path_buf(),
        })
    }

    /// The outcome, once the build has ended.
    pub fn finished(&mut self) -> Option<Outcome> {
        let status = self.child.try_wait().ok()??;
        // A program it started may hold the pipe open, so this doesn't wait long.
        let deadline = Instant::now() + LAST_OUTPUT;
        while !self.reader.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let bytes = self.output.lock().unwrap();
        let output = String::from_utf8_lossy(&bytes);
        let (errors, warnings) = problems(&output, &self.name);
        let last_line = last_word(&output);
        Some(Outcome {
            ok: status.success(),
            errors,
            warnings,
            last_line,
            took: self.started.elapsed(),
        })
    }

    /// Stops the build and everything it started.
    pub fn stop(&mut self) {
        if let Some(pid) = rustix::process::Pid::from_raw(self.child.id() as i32) {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::TERM);
        }
        let _ = self.child.wait();
    }
}

/// The file to build for `path`: the root a `% !TEX root =` comment names, or
/// failing that, for a chapter with no `\documentclass`, a `main.tex` beside it
/// or a folder up. Otherwise the file itself.
pub fn root_of(path: &Path, text: &str) -> PathBuf {
    let dir = path.parent().unwrap_or(Path::new("."));
    let lines = text.lines().take(10);
    let named = lines.filter_map(|l| {
        let comment = l.trim_start().strip_prefix('%')?.trim_start();
        let (key, value) = comment.split_once('=')?;
        key.to_ascii_lowercase()
            .trim()
            .eq("!tex root")
            .then(|| value.trim().to_string())
    });
    if let Some(root) = named.into_iter().next() {
        return dir.join(root);
    }
    if !text.contains("\\documentclass") {
        for candidate in [dir.join("main.tex"), dir.join("../main.tex")] {
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_are_read_from_file_line_output() {
        let output = "This is pdfTeX\n./paper.tex:4: Undefined control sequence.\nl.4 \\foo\n\n./sub/ch1.tex:12: LaTeX Error: File `x.sty' not found.\nnot:a: problem\nC:\\path\n";
        let (errors, warnings) = problems(output, "paper.tex");
        assert_eq!(
            errors,
            [
                Problem {
                    file: "paper.tex".to_string(),
                    line: 4,
                    text: "Undefined control sequence.".to_string(),
                },
                Problem {
                    file: "sub/ch1.tex".to_string(),
                    line: 12,
                    text: "LaTeX Error: File `x.sty' not found.".to_string(),
                },
            ]
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn the_same_error_from_several_passes_shows_once() {
        let output = "./a.tex:1: Boom\n./a.tex:1: Boom\n";
        assert_eq!(problems(output, "a.tex").0.len(), 1);
    }

    /// From real `latexmk` output: a paper with a bad command, a mismatched
    /// environment and an undefined citation and label.
    const REAL_BAD: &str = r#"Running 'pdflatex  -interaction=nonstopmode -file-line-error -recorder  "bad.tex"'
This is pdfTeX, Version 3.141592653-2.6-1.40.29 (TeX Live 2026) (preloaded format=pdflatex)
./bad.tex:4: Undefined control sequence.
l.4 This uses \notacommand
                           here.

LaTeX Warning: Citation `missingkey' on page 1 undefined on input line 5.

LaTeX Warning: Reference `sec:none' on page 1 undefined on input line 5.

./bad.tex:8: LaTeX Error: \begin{itemize} on input line 6 ended by \end{enumerate}.
LaTeX Warning: There were undefined references.
Latexmk: Errors, so I did not complete making targets
  Citation `missingkey' on page 1 undefined on input line 5
Collected error summary (may duplicate other messages):
  pdflatex: Command for 'pdflatex' gave return code 1
      Refer to 'bad.log' and/or above output for details

Latexmk: Sometimes, the -f option can be used to get latexmk
  latexmk after you've corrected the files.
"#;

    /// From real `latexmk` output: a package that isn't installed.
    const REAL_MISSING: &str = r#"! LaTeX Error: File `nosuchpackage.sty' not found.

Type X to quit or <RETURN> to proceed,
or enter new name. (Default extension: sty)

Enter file name: 
./missing.tex:3: Emergency stop.
<read *> 
         
l.3 \begin
          {document}^^M
./missing.tex:3:  ==> Fatal error occurred, no output PDF file produced!
Transcript written on missing.log.
Collected error summary (may duplicate other messages):
  pdflatex: Command for 'pdflatex' gave return code 1
"#;

    #[test]
    fn real_errors_and_warnings_are_read() {
        let (errors, warnings) = problems(REAL_BAD, "bad.tex");
        let found: Vec<String> = errors.iter().map(Problem::label).collect();
        assert_eq!(
            found,
            [
                "bad.tex:4  Undefined control sequence.",
                "bad.tex:8  LaTeX Error: \\begin{itemize} on input line 6 ended by \\end{enumerate}.",
            ]
        );
        let found: Vec<String> = warnings.iter().map(Problem::label).collect();
        assert_eq!(
            found,
            [
                "bad.tex:5  Citation `missingkey' on page 1 undefined",
                "bad.tex:5  Reference `sec:none' on page 1 undefined",
            ]
        );
    }

    #[test]
    fn warnings_a_later_pass_settled_dont_count() {
        let output = "This is pdfTeX, Version 3.1\nLaTeX Warning: Citation `a' on page 1 undefined on input line 4.\nThis is BibTeX, Version 0.99\nThis is pdfTeX, Version 3.1\nOutput written on a.pdf\n";
        assert!(problems(output, "a.tex").1.is_empty());
        let still =
            format!("{output}LaTeX Warning: Citation `b' on page 1 undefined on input line 5.\n");
        assert_eq!(problems(&still, "a.tex").1.len(), 1);
    }

    #[test]
    fn a_missing_package_is_named_at_the_place_latex_stopped() {
        let (errors, _) = problems(REAL_MISSING, "missing.tex");
        let found: Vec<String> = errors.iter().map(Problem::label).collect();
        assert_eq!(
            found,
            ["missing.tex:3  LaTeX Error: File `nosuchpackage.sty' not found."]
        );
    }

    #[test]
    fn an_emergency_stop_alone_is_still_reported() {
        let (errors, _) = problems("./a.tex:9: Emergency stop.\n", "a.tex");
        assert_eq!(errors[0].label(), "a.tex:9  Emergency stop.");
    }

    #[test]
    fn a_failure_is_described_by_what_latexmk_collected_not_its_advice() {
        assert_eq!(
            last_word(REAL_BAD),
            "pdflatex: Command for 'pdflatex' gave return code 1"
        );
        assert_eq!(last_word("one\n  two  \n\n"), "two");
        assert_eq!(last_word(""), "");
    }

    #[test]
    fn labels_say_where_when_they_can() {
        let p = Problem {
            file: "a.tex".to_string(),
            line: 3,
            text: "Boom".to_string(),
        };
        assert_eq!(p.label(), "a.tex:3  Boom");
        assert_eq!(Problem { line: 0, ..p }.label(), "a.tex  Boom");
    }

    #[test]
    fn file_names_are_quoted_for_the_shell() {
        assert_eq!(filled("run %f now", "a b.tex"), "run 'a b.tex' now");
        assert_eq!(filled("run", "it's.tex"), "run 'it'\\''s.tex'");
    }

    #[test]
    fn the_default_needs_latexmk_and_a_custom_command_doesnt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap().to_string();
        assert_eq!(command_for(None, "a.tex", &path), None);
        std::fs::write(dir.path().join("latexmk"), "").unwrap();
        let found = command_for(None, "a.tex", &path).unwrap();
        assert!(found.starts_with("latexmk -pdf"), "{found}");
        assert!(found.ends_with("'a.tex'"), "{found}");
        assert_eq!(
            command_for(Some("tectonic %f"), "a.tex", "").unwrap(),
            "tectonic 'a.tex'"
        );
    }

    #[test]
    fn the_root_comes_from_a_comment_then_a_main_file() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("ch");
        std::fs::create_dir(&sub).unwrap();
        let chapter = sub.join("one.tex");
        assert_eq!(
            root_of(&chapter, "% !TEX root = ../thesis.tex\n"),
            sub.join("../thesis.tex")
        );
        assert_eq!(
            root_of(&chapter, "%!TeX root=../thesis.tex\n"),
            sub.join("../thesis.tex")
        );
        assert_eq!(root_of(&chapter, "Text.\n"), chapter);
        std::fs::write(dir.path().join("main.tex"), "").unwrap();
        assert_eq!(root_of(&chapter, "Text.\n"), sub.join("../main.tex"));
        let paper = "\\documentclass{article}\n";
        assert_eq!(
            root_of(&chapter, paper),
            chapter,
            "a whole paper is its own root"
        );
    }

    #[test]
    fn a_build_reports_its_output_and_exit() {
        let dir = tempfile::tempdir().unwrap();
        let script = "echo './a.tex:2: Boom'; echo to-stderr >&2; exit 1";
        let mut b = Build::start(script, dir.path(), "a.tex").unwrap();
        let outcome = loop {
            if let Some(o) = b.finished() {
                break o;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        assert!(!outcome.ok);
        assert_eq!(outcome.errors.len(), 1);
        assert_eq!(outcome.last_line, "to-stderr");
    }

    #[test]
    fn stopping_ends_what_the_build_started() {
        let dir = tempfile::tempdir().unwrap();
        let mark = dir.path().join("ran");
        let script = format!("(sleep 0.5; touch '{}') & wait", mark.display());
        let mut b = Build::start(&script, dir.path(), "a.tex").unwrap();
        std::thread::sleep(Duration::from_millis(50));
        b.stop();
        std::thread::sleep(Duration::from_millis(800));
        assert!(!mark.exists(), "the child kept running");
    }
}
