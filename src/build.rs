//! Building a paper's PDF. The build runs beside the editor, so typing carries
//! on, and its output is read for problems to jump to.

use std::collections::HashMap;
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
/// Past this a source file isn't read to check a warning.
const MAX_SOURCE_BYTES: u64 = 8 * 1024 * 1024;

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

/// The kinds of file a person might have to fix.
const SOURCES: [&str; 6] = [".tex", ".ltx", ".sty", ".cls", ".bib", ".bbl"];

/// Whether `name` is a file a person might have to fix.
fn is_source(name: &str) -> bool {
    SOURCES.iter().any(|t| name.ends_with(t))
}

/// The local source file named at the start of `text`, the rest of a line after
/// a `(`. A name can hold spaces, so it runs through its extension. TeX quotes
/// some names.
fn source_name(text: &str) -> Option<&str> {
    let text = text.strip_prefix('"').unwrap_or(text);
    if !text.starts_with("./") && !text.starts_with("../") {
        return None;
    }
    let text = &text[..text.find(['(', ')', '"']).unwrap_or(text.len())];
    text.match_indices('.').find_map(|(at, _)| {
        let end = at + SOURCES.iter().find(|t| text[at..].starts_with(**t))?.len();
        let ended = text[end..]
            .chars()
            .next()
            .is_none_or(|c| matches!(c, ' ' | '['));
        ended.then(|| &text[..end])
    })
}

/// Document text TeX is echoing, which needn't balance.
#[derive(Clone, Copy, PartialEq)]
enum Echo {
    /// The text of an `\\hbox` that didn't fit.
    Box,
    /// After `Runaway argument?`, up to the error it leads to.
    Runaway,
}

/// Whether `line` comes after the text of a box. TeX often goes straight on to
/// its next record with no blank line: a page, or a file ending or starting.
/// Box text itself starts with a font or `[]`.
fn ends_box_text(line: &str) -> bool {
    let mut chars = line.chars();
    match chars.next() {
        None | Some('(' | ')') => true,
        Some('[') => chars.next().is_some_and(|c| c.is_ascii_digit()),
        Some(_) => line.trim().is_empty(),
    }
}

/// Follows which file TeX is reading. It prints `(` and a name on entering a
/// file and `)` on leaving, and other text has parentheses too, so all of them
/// are tracked, with a name only where there is one.
#[derive(Default)]
struct Files {
    open: Vec<Option<String>>,
    echo: Option<Echo>,
}

impl Files {
    /// The file being read, as the build named it.
    fn current(&self) -> Option<&str> {
        let file = self.open.iter().rev().flatten().next()?;
        Some(file.strip_prefix("./").unwrap_or(file))
    }

    fn read(&mut self, line: &str) {
        match self.echo {
            Some(Echo::Box) if !ends_box_text(line) => return,
            Some(Echo::Runaway) if !line.starts_with('!') && error_line(line).is_none() => return,
            _ => self.echo = None,
        }
        // Only an `\\hbox` shows its text; a `\\vbox` reports on one line.
        if line.starts_with("Overfull \\hbox") || line.starts_with("Underfull \\hbox") {
            self.echo = Some(Echo::Box);
            return;
        }
        if line.starts_with("Runaway argument?") || line.starts_with("Runaway text?") {
            self.echo = Some(Echo::Runaway);
            return;
        }
        // Around an error TeX echoes the document, set off like this.
        if line.starts_with("l.") || line.starts_with(' ') || line.starts_with('<') {
            return;
        }
        let mut skip_to = 0;
        for (i, c) in line.char_indices() {
            if i < skip_to {
                continue;
            }
            match c {
                '(' => {
                    let name = source_name(&line[i + 1..]);
                    skip_to = name.map_or(0, |n| i + 1 + n.len());
                    self.open.push(name.map(str::to_string));
                }
                ')' => {
                    self.open.pop();
                }
                _ => {}
            }
        }
    }
}

/// A `file:line: message` line, the form `-file-line-error` gives.
fn error_line(line: &str) -> Option<Problem> {
    let (file, rest) = line.split_once(':')?;
    let (number, text) = rest.split_once(':')?;
    if !is_source(file) {
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
fn undefined(line: &str, file: &str) -> Option<Problem> {
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
        file: file.to_string(),
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
    let mut files = Files::default();
    for line in output.lines() {
        if line.starts_with("This is ") {
            files = Files::default();
        }
        let file = files.current().unwrap_or(root);
        if let Some(text) = line.strip_prefix("! ") {
            let p = Problem {
                file: file.to_string(),
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
        } else if let Some(p) = undefined(line, file) {
            // A first pass warns about every citation; later ones settle them.
            let from = line.as_ptr() as usize - output.as_ptr() as usize;
            if from >= last_pass && !warnings.contains(&p) {
                warnings.push(p);
            }
        }
        files.read(line);
    }
    (errors, warnings)
}

/// The source files `output` mentions, as the build named them.
fn sources_in(output: &str) -> Vec<&str> {
    let mut found = Vec::new();
    for line in output.lines() {
        for (at, _) in line.match_indices('(') {
            if let Some(name) = source_name(&line[at + 1..]) {
                if !found.contains(&name) {
                    found.push(name);
                }
            }
        }
    }
    found
}

/// The lines of a source file, if it's small enough to read on the way past. A
/// path in a log can be anything, so only a regular file is opened.
fn source_lines(path: &Path) -> Option<Vec<String>> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_SOURCE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    Some(text.lines().map(str::to_string).collect())
}

/// Checks each warning against the source. The file a warning came from is
/// only a reading of TeX's parentheses, and text it prints can throw that off,
/// so `Citation `key' ... on input line 12` is looked for at line 12, in the
/// file it's said to be in and then in the others the build read. TeX gives the
/// line a command ended on, so the two before it count too.
pub fn check_against_source(warnings: &mut [Problem], output: &str, dir: &Path) {
    let names = sources_in(output);
    let mut read: HashMap<String, Option<Vec<String>>> = HashMap::new();
    for warning in warnings.iter_mut().filter(|w| w.line > 0) {
        let Some((_, key)) = warning.text.split_once('`') else {
            continue;
        };
        let key = key.split('\'').next().unwrap_or(key);
        let line = warning.line;
        let mut holds = |file: &str| {
            let lines = read
                .entry(file.to_string())
                .or_insert_with(|| source_lines(&dir.join(file)));
            lines.as_ref().is_some_and(|lines| {
                let near = lines.get(line.saturating_sub(3)..line.min(lines.len()));
                near.is_some_and(|near| near.iter().any(|l| l.contains(key)))
            })
        };
        if holds(&warning.file) {
            continue;
        }
        let local = names.iter().map(|n| n.strip_prefix("./").unwrap_or(n));
        if let Some(file) = local.into_iter().find(|n| holds(n)) {
            warning.file = file.to_string();
        }
    }
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
        let (errors, mut warnings) = problems(&output, &self.name);
        check_against_source(&mut warnings, &output, &self.dir);
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

    /// From real `latexmk` output: a paper with chapters, one nested, one
    /// `\\include`d, and parentheses in ordinary text.
    const REAL_CHAPTERS: &str = r#"This is pdfTeX, Version 3.141592653-2.6-1.40.29 (TeX Live 2026) (preloaded format=pdflatex)
 restricted \write18 enabled.
entering extended mode
(./main.tex
LaTeX2e <2026-06-01>
(/usr/local/texlive/2026/texmf-dist/tex/latex/base/article.cls
Document Class: article 2025/01/22 v1.4n Standard LaTeX document class
(/usr/local/texlive/2026/texmf-dist/tex/latex/base/size10.clo)) (/usr/local/texlive/2026/texmf-dist/tex/latex/l3backend/l3backend-pdftex.def) (./main.aux (./chapters/two.aux))

LaTeX Warning: Citation `inroot' on page 1 undefined on input line 4.

(./chapters/intro.tex

LaTeX Warning: Citation `inchapter' on page 1 undefined on input line 3.


LaTeX Warning: Reference `nolabel' on page 1 undefined on input line 3.

(./chapters/deep.tex

LaTeX Warning: Citation `indeep' on page 1 undefined on input line 2.

))

LaTeX Warning: Citation `afterch' on page 1 undefined on input line 6.

[1{/usr/local/texlive/2026/texmf-var/fonts/map/pdftex/updmap/pdftex.map}] (./chapters/two.tex

LaTeX Warning: Citation `intwo' on page 2 undefined on input line 3.

./chapters/two.tex:4: Undefined control sequence.
l.4 \notacommand (unbalanced
                 
) [2] (./main.aux (./chapters/two.aux))

LaTeX Warning: Citation `atend' on page 2 undefined on input line 9.

"#;

    /// From real `latexmk` output: text that overflows its box, with a stray
    /// `)`, and included files with spaces in their names.
    const REAL_BOXES: &str = r#"This is pdfTeX, Version 3.141592653-2.6-1.40.29 (TeX Live 2026) (preloaded format=pdflatex)
(./main.tex
LaTeX2e <2026-06-01>
(./ch/boxes.tex
Overfull \hbox (258.99226pt too wide) detected at line 1
\OT1/cmr/m/n/10 Smith) says something long enough to overflow the box width really

Underfull \hbox (badness 10000) detected at line 2
\OT1/cmr/m/n/10 a) b

Overfull \hbox (255.50113pt too wide) in paragraph at lines 3--6
[][] 

Overfull \hbox (31.83414pt too wide) in paragraph at lines 7--8
\OT1/cmr/m/n/10 Paragraph with a very-long-unbreakable-word Smith) Supercalifragilisticexpialidocious-

LaTeX Warning: Citation `inboxes' on page 1 undefined on input line 9.

)

LaTeX Warning: Citation `aftersboxes' on page 1 undefined on input line 4.

(./ch/my chapter.tex

LaTeX Warning: Citation `inspaced' on page 1 undefined on input line 2.

)

LaTeX Warning: Citation `afterspace' on page 1 undefined on input line 6.

[1{/usr/local/texlive/2026/texmf-var/fonts/map/pdftex/updmap/pdftex.map}]

LaTeX Warning: Citation `lastroot' on page 2 undefined on input line 10.

[2] (./main.aux)
"#;

    /// From real `latexmk` output: an argument that never closes.
    const REAL_RUNAWAY: &str = r#"This is pdfTeX, Version 3.141592653-2.6-1.40.29 (TeX Live 2026) (preloaded format=pdflatex)
(./run.tex
(./ch/run.tex
Runaway argument?
{Smith) unclosed argument \par \cite {inrun} 
./ch/run.tex:4: File ended while scanning use of \foo.
<inserted text> 
                \par 
l.4 \input{ch/run}
                  
LaTeX Warning: Citation `here' on page 1 undefined on input line 2.

)

LaTeX Warning: Citation `afterrun' on page 1 undefined on input line 5.

"#;

    /// From real `latexmk` output: box text with no blank line after it, before
    /// a page, a file closing and a file opening, and a `\\vbox` with no text.
    const REAL_TIGHT: &str = r#"This is pdfTeX, Version 3.141592653-2.6-1.40.29 (TeX Live 2026) (preloaded format=pdflatex)
(./main.tex
LaTeX2e <2026-06-01>
(./ch/endsoverfull.tex
Overfull \hbox (76.16762pt too wide) in paragraph at lines 1--5
\OT1/cmr/m/n/10 Ends on an over-full para-graph Supercalifragilisticexpialidocious-Supercalifragi
)

LaTeX Warning: Citation `afterfirst' on page 1 undefined on input line 4.

[1{/usr/local/texlive/2026/texmf-var/fonts/map/pdftex/updmap/pdftex.map}] (./ch/endsbox.tex

LaTeX Warning: Citation `inendsbox' on page 2 undefined on input line 1.


Overfull \hbox (234.74217pt too wide) detected at line 2
\OT1/cmr/m/n/10 Smith) last thing in the file and it is much too wide for the box
)

LaTeX Warning: Citation `aftersecond' on page 2 undefined on input line 7.

[2] (./ch/inc.tex

LaTeX Warning: Citation `ininc' on page 3 undefined on input line 1.


Overfull \hbox (63.90358pt too wide) in paragraph at lines 1--3
[]\OT1/cmr/m/n/10 [[]] In-clude ends over-full Supercalifragilisticexpialidocious-Supercalifragil
[3])

LaTeX Warning: Citation `afterinc' on page 4 undefined on input line 9.

[4]
Overfull \hbox (177.43654pt too wide) detected at line 11
\OT1/cmr/m/n/10 Smith) overfull in a box at the very end of a page

Underfull \vbox (badness 10000) detected at line 11
[5] (./ch/tail.tex [6] (./ch/tail2.tex

LaTeX Warning: Citation `intail2' on page 7 undefined on input line 1.

))

LaTeX Warning: Citation `aftertail' on page 7 undefined on input line 14.

"#;

    fn places(warnings: &[Problem]) -> Vec<String> {
        let place = |p: &Problem| format!("{}:{}", p.file, p.line);
        warnings.iter().map(place).collect()
    }

    #[test]
    fn a_stray_parenthesis_in_a_box_doesnt_end_the_file() {
        let (_, warnings) = problems(REAL_BOXES, "main.tex");
        assert_eq!(
            places(&warnings),
            [
                "ch/boxes.tex:9",
                "main.tex:4",
                "ch/my chapter.tex:2",
                "main.tex:6",
                "main.tex:10",
            ]
        );
    }

    #[test]
    fn box_text_ends_where_the_next_record_starts_blank_line_or_not() {
        let (_, warnings) = problems(REAL_TIGHT, "main.tex");
        assert_eq!(
            places(&warnings),
            [
                "main.tex:4",
                "ch/endsbox.tex:1",
                "main.tex:7",
                "ch/inc.tex:1",
                "main.tex:9",
                "ch/tail2.tex:1",
                "main.tex:14",
            ]
        );
    }

    #[test]
    fn a_stray_parenthesis_in_runaway_text_doesnt_end_the_file() {
        let (errors, warnings) = problems(REAL_RUNAWAY, "run.tex");
        assert_eq!(places(&warnings), ["ch/run.tex:2", "run.tex:5"]);
        assert_eq!(errors[0].file, "ch/run.tex");
    }

    #[test]
    fn a_box_with_nothing_to_show_doesnt_hide_the_next_line() {
        let output = "(./a.tex\nUnderfull \\vbox (badness 10000) has occurred while \\output is active []\n(./ch.tex\nLaTeX Warning: Citation `x' on page 1 undefined on input line 2.\n";
        assert_eq!(places(&problems(output, "a.tex").1), ["ch.tex:2"]);
    }

    #[test]
    fn a_warning_is_checked_against_the_source() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("ch")).unwrap();
        std::fs::write(dir.path().join("main.tex"), "one\ntwo\nthree\nfour\n").unwrap();
        std::fs::write(dir.path().join("ch/my a.tex"), "x\ny\nz\nw\n\\cite{key}\n").unwrap();
        // A stray `)` from `\typeout` closes the chapter early.
        let output = "This is pdfTeX\n(./main.tex\n(./ch/my a.tex\na stray)\nLaTeX Warning: Citation `key' on page 1 undefined on input line 5.\n)\n)\n";
        let (_, mut warnings) = problems(output, "main.tex");
        assert_eq!(warnings[0].file, "main.tex", "what the parentheses say");
        check_against_source(&mut warnings, output, dir.path());
        assert_eq!(places(&warnings), ["ch/my a.tex:5"]);
    }

    #[test]
    fn a_key_on_the_lines_before_counts_and_a_key_found_nowhere_stays() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.tex"), "\\cite{%\nkey\n}\n").unwrap();
        std::fs::write(dir.path().join("b.tex"), "key\n").unwrap();
        let warning = |line, key: &str| Problem {
            file: "a.tex".to_string(),
            line,
            text: format!("Citation `{key}' on page 1 undefined"),
        };
        let output = "(./a.tex (./b.tex";
        let mut kept = [warning(3, "key"), warning(3, "gone"), warning(99, "key")];
        check_against_source(&mut kept, output, dir.path());
        assert_eq!(kept.map(|w| w.file), ["a.tex", "a.tex", "a.tex"]);
    }

    #[test]
    fn a_source_that_isnt_a_plain_file_isnt_read() {
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/dev/zero", dir.path().join("z.tex")).unwrap();
        assert!(source_lines(&dir.path().join("z.tex")).is_none());
        assert!(source_lines(&dir.path().join("none.tex")).is_none());
        let mut w = [Problem {
            file: "z.tex".to_string(),
            line: 1,
            text: "Citation `k' on page 1 undefined".to_string(),
        }];
        check_against_source(&mut w, "(./z.tex", dir.path());
        assert_eq!(w[0].file, "z.tex");
    }

    #[test]
    fn source_names_can_hold_spaces_and_quotes() {
        assert_eq!(source_name("./my chapter.tex"), Some("./my chapter.tex"));
        assert_eq!(
            source_name("\"./my chapter.tex\""),
            Some("./my chapter.tex")
        );
        assert_eq!(source_name("../up.tex [1]"), Some("../up.tex"));
        assert_eq!(source_name("./a.tex)"), Some("./a.tex"));
        assert_eq!(source_name("./a.tex (./b.tex"), Some("./a.tex"));
        assert_eq!(source_name("./a.aux"), None);
        assert_eq!(source_name("./my.texture"), None);
        assert_eq!(source_name("Default extension: sty)"), None);
        assert_eq!(source_name("see the transcript"), None);
        assert_eq!(source_name("/usr/share/a.tex"), None);
    }

    #[test]
    fn a_warning_belongs_to_the_file_tex_was_reading() {
        let (errors, warnings) = problems(REAL_CHAPTERS, "main.tex");
        let found: Vec<String> = warnings
            .iter()
            .map(|p| format!("{}:{}", p.file, p.line))
            .collect();
        assert_eq!(
            found,
            [
                "main.tex:4",
                "chapters/intro.tex:3",
                "chapters/intro.tex:3",
                "chapters/deep.tex:2",
                "main.tex:6",
                "chapters/two.tex:3",
                "main.tex:9",
            ]
        );
        assert_eq!(
            errors[0].label(),
            "chapters/two.tex:4  Undefined control sequence."
        );
    }

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
