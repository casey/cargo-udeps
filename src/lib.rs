mod defs;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::OsString;
use std::fmt::Write as _;
use std::io::Write;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::env;

use ansi_term::Colour;
use cargo::core::compiler::{DefaultExecutor, Executor, Unit};
use cargo::core::resolver::ResolveOpts;
use cargo::core::manifest::Target;
use cargo::core::package_id::PackageId;
use cargo::core::shell::Shell;
use cargo::core::{InternedString, Package, Resolve};
use cargo::ops::Packages;
use cargo::util::command_prelude::{ArgMatchesExt, CompileMode, ProfileChecking};
use cargo::util::process_builder::ProcessBuilder;
use cargo::{CargoResult, CliError, CliResult, Config};
use structopt::StructOpt;
use structopt::clap::{AppSettings, ArgMatches};

use crate::defs::CrateSaveAnalysis;

pub fn run<I: IntoIterator<Item = OsString>, W: Write>(args :I, config :&mut Config, stdout: W) -> CliResult {
	let args = args.into_iter().collect::<Vec<_>>();
	let Opt::Udeps(opt) = Opt::from_iter_safe(&args)?;
	let clap_matches = Opt::clap().get_matches_from_safe(args)?;
	cargo::core::maybe_allow_nightly_features();
	match opt.run(config, stdout, clap_matches.subcommand_matches("udeps").unwrap())? {
		0 => Ok(()),
		code => Err(CliError::code(code)),
	}
}

#[derive(StructOpt, Debug)]
#[structopt(
	about,
	bin_name = "cargo",
	global_setting(AppSettings::DeriveDisplayOrder),
)]
enum Opt {
	#[structopt(
		about,
		name = "udeps",
		after_help(
			"\
If the `--package` argument is given, then SPEC is a package ID specification
which indicates which package should be built. If it is not given, then the
current package is built. For more information on SPEC and its format, see the
`cargo help pkgid` command.

All packages in the workspace are checked if the `--workspace` flag is supplied. The
`--workspace` flag is automatically assumed for a virtual manifest.
Note that `--exclude` has to be specified in conjunction with the `--workspace` flag.

Compilation can be configured via the use of profiles which are configured in
the manifest. The default profile for this command is `dev`, but passing
the `--release` flag will use the `release` profile instead.

The `--profile test` flag can be used to check unit tests with the
`#[cfg(test)]` attribute."
		)
    )]
	Udeps(OptUdeps),
}

#[derive(StructOpt, Debug)]
struct OptUdeps {
	#[structopt(short, long, help("No output printed to stdout"))]
	quiet: bool,
	#[structopt(long, help("Alias for --workspace (deprecated)"))]
	all: bool,
	#[structopt(long, help("Check all packages in the workspace"))]
	workspace: bool,
	#[structopt(long, help("Check only the specified binary"))]
	lib: bool,
	#[structopt(long, help("Check all binaries"))]
	bins: bool,
	#[structopt(long, help("Check all examples"))]
	examples: bool,
	#[structopt(long, help("Check all tests"))]
	tests: bool,
	#[structopt(long, help("Check all benches"))]
	benches: bool,
	#[structopt(long, help("Check all targets"))]
	all_targets: bool,
	#[structopt(long, help("Check artifacts in release mode, with optimizations"))]
	release: bool,
	#[structopt(long, help("Activate all available features"))]
	all_features: bool,
	#[structopt(long, help("Do not activate the `default` feature"))]
	no_default_features: bool,
	#[structopt(long, help("Require Cargo.lock and cache are up to date"))]
	frozen: bool,
	#[structopt(long, help("Require Cargo.lock is up to date"))]
	locked: bool,
	#[structopt(long, help("Run without accessing the network"))]
	offline: bool,
	#[structopt(
		short,
		long,
		parse(from_occurrences),
		help("Use verbose output (-vv very verbose/build.rs output)")
	)]
	verbose: u64,
	#[structopt(
		short,
		long,
		value_name("SPEC"),
		min_values(1),
		number_of_values(1),
		help("Package(s) to check")
	)]
	package: Vec<String>,
	#[structopt(
		long,
		value_name("SPEC"),
		min_values(1),
		number_of_values(1),
		help("Exclude packages from the check")
	)]
	exclude: Vec<String>,
	#[structopt(
		short,
		long,
		value_name("N"),
		help("Number of parallel jobs, defaults to # of CPUs")
	)]
	jobs: Option<String>,
	#[structopt(
		long,
		value_name("NAME"),
		min_values(0),
		number_of_values(1),
		help("Check only the specified bin target")
	)]
	bin: Vec<String>,
	#[structopt(
		long,
		value_name("NAME"),
		min_values(0),
		number_of_values(1),
		help("Check only the specified example target")
	)]
	example: Vec<String>,
	#[structopt(
		long,
		value_name("NAME"),
		min_values(0),
		number_of_values(1),
		help("Check only the specified test target")
	)]
	test: Vec<String>,
	#[structopt(
		long,
		value_name("NAME"),
		min_values(0),
		number_of_values(1),
		help("Check only the specified bench target")
	)]
	bench: Vec<String>,
	#[structopt(
		long,
		value_name("PROFILE"),
		help("Profile to build the selected target for")
	)]
	profile: Option<String>,
	#[structopt(
		long,
		value_name("FEATURES"),
		min_values(1),
		help("Space-separated list of features to activate")
	)]
	features: Vec<String>,
	#[structopt(long, value_name("TRIPLE"), help("Check for the target triple"))]
	target: Option<String>,
	#[structopt(
		long,
		value_name("DIRECTORY"),
		help("Directory for all generated artifacts")
	)]
	target_dir: Option<PathBuf>,
	#[structopt(long, value_name("PATH"), help("Path to Cargo.toml"))]
	manifest_path: Option<PathBuf>,
	#[structopt(
		long,
		value_name("FMT"),
		case_insensitive(true),
		possible_values(&["human", "json", "short"]),
		default_value("human"),
		help("Error format")
	)]
	message_format: String,
	#[structopt(
		long,
		value_name("WHEN"),
		case_insensitive(false),
		possible_values(&["auto", "always", "never"]),
		help("Coloring")
	)]
	color: Option<String>,
}

impl OptUdeps {
	fn run<W: Write>(
		&self,
		config :&mut Config,
		mut stdout :W,
		clap_matches :&ArgMatches
	) -> CargoResult<i32> {
		if self.verbose > 0 {
			let mut shell = config.shell();
			shell.warn(
				"currently verbose command informations (\"Running `..`\") are not correct.",
			)?;
			shell.warn("for example, `cargo-udeps` does these modifications:")?;
			shell.warn("- changes `$CARGO` to the value given from `cargo`")?;
			shell.warn("- sets `$RUST_CONFIG_SAVE_ANASYSIS` (for crates on the local filesystem)")?;
			shell.warn("- adds `-Z save-analysis` (〃)")?;
		}

		config.configure(
			match self.verbose {
				0 => 0,
				1 => 1,
				_ => 2,
			},
			if self.quiet { Some(true) } else { None }, // https://docs.rs/cargo/0.39.0/src/cargo/util/config.rs.html#602-604
			&self.color,
			self.frozen,
			self.locked,
			self.offline,
			&self.target_dir,
			&[],
		)?;
		let ws = clap_matches.workspace(config)?;
		let test = match self.profile.as_ref().map(Deref::deref) {
			None => false,
			Some("test") => true,
			Some(profile) => return Err(failure::format_err!(
				"unknown profile: `{}`, only `test` is currently supported",
				profile,
			)),
		};
		let mode = CompileMode::Check { test };
		let pc = ProfileChecking::Unchecked;
		let compile_opts = clap_matches.compile_options(config, mode, Some(&ws), pc)?;

		let opts = ResolveOpts::new(
			/*dev_deps*/ true,
			&self.features,
			self.all_features,
			!self.no_default_features,

		);
		let ws_resolve = cargo::ops::resolve_ws_with_opts(
			&ws,
			opts,
			&Packages::All.to_package_id_specs(&ws)?,
		)?;

		let packages = ws_resolve.pkg_set
			.get_many(ws_resolve.pkg_set.package_ids())?
			.into_iter()
			.map(|p| (p.package_id(), p))
			.collect::<HashMap<_, _>>();

		let dependency_names = ws
			.members()
			.map(|from| {
				let val = DependencyNames::new(from, &packages, &ws_resolve.targeted_resolve, &mut config.shell())?;
				let key = from.package_id();
				Ok((key, val))
			})
			.collect::<CargoResult<HashMap<_, _>>>()?;

		let data = Arc::new(Mutex::new(ExecData::new(config)?));
		let exec :Arc<dyn Executor + 'static> = Arc::new(Exec { data : data.clone() });
		cargo::ops::compile_with_exec(&ws, &compile_opts, &exec)?;
		let data = data.lock().unwrap();

		let mut used_normal_dev_dependencies = HashSet::new();
		let mut used_build_dependencies = HashSet::new();
		let mut normal_dev_dependencies = dependency_names
			.iter()
			.flat_map(|(&m, d)| d.normal_dev_non_lib.iter().map(move |&s| (m, s)))
			.collect::<HashSet<_>>();
		let mut build_dependencies = dependency_names
			.iter()
			.flat_map(|(&m, d)| d.build_non_lib.iter().map(move |&s| (m, s)))
			.collect::<HashSet<_>>();

		for cmd_info in data.relevant_cmd_infos.iter() {
			let analysis = cmd_info.get_save_analysis(&mut config.shell())?;
			// may not be workspace member
			if let Some(dependency_names) = dependency_names.get(&cmd_info.pkg) {
				let (
					by_extern_crate_name,
					by_lib_true_snakecased_name,
					used_dependencies,
					dependencies
				) = if cmd_info.custom_build {
					(
						&dependency_names.build_by_extern_crate_name,
						&dependency_names.build_by_lib_true_snakecased_name,
						&mut used_build_dependencies,
						&mut build_dependencies,
					)
				} else {
					(
						&dependency_names.normal_dev_by_extern_crate_name,
						&dependency_names.normal_dev_by_lib_true_snakecased_name,
						&mut used_normal_dev_dependencies,
						&mut normal_dev_dependencies,
					)
				};
				for ext in &analysis.prelude.external_crates {
					if let Some(dependency_names) = by_lib_true_snakecased_name.get(&ext.id.name) {
						for dependency_name in dependency_names {
							used_dependencies.insert((cmd_info.pkg, *dependency_name));
						}
					}
				}
				for (name, _) in &cmd_info.externs {
					// We ignore the `lib` that `bin`s, `example`s, and `test`s in the same
					// `Package` depend on.
					if let Some(dependency_name) = by_extern_crate_name.get(name) {
						dependencies.insert((cmd_info.pkg, *dependency_name));
					}
				}
			}
		}

		let mut unused_dependencies = BTreeMap::new();
		for (dependencies, used_dependencies, custom_build) in &[
			(&normal_dev_dependencies, &used_normal_dev_dependencies, false),
			(&build_dependencies, &used_build_dependencies, true),
		] {
			for (id, dependency) in *dependencies {
				if !used_dependencies.contains(&(*id, *dependency)) {
					let (normal_dev, build) = unused_dependencies
						.entry(id)
						.or_insert_with(|| (BTreeSet::new(), BTreeSet::new()));
					if *custom_build {
						build.insert(dependency);
					} else {
						normal_dev.insert(dependency);
					}
				}
			}
		}
		if !unused_dependencies.values().all(|(ps1, ps2)| ps1.is_empty() && ps2.is_empty()) {
			writeln!(stdout, "unused dependencies:")?;

			for (member, (normal_dev_dependencies, build_dependencies)) in unused_dependencies {
				writeln!(stdout, "`{}`", member)?;
				let (edge, joint) = if build_dependencies.is_empty() {
					(' ', '└')
				} else {
					('│', '├')
				};
				for (dependencies, edge, joint, prefix) in &[
					(normal_dev_dependencies, edge, joint, "(dev-)"),
					(build_dependencies, ' ', '└', "build-"),
				] {
					if !dependencies.is_empty() {
						writeln!(stdout, "{}─── {}dependencies", joint, prefix)?;
						let mut dependencies = dependencies.iter().peekable();
						while let Some(dependency) = dependencies.next() {
							let joint = if dependencies.peek().is_some() {
								'├'
							} else {
								'└'
							};
							writeln!(stdout, "{}    {}─── {:?}", edge, joint, dependency)?;
						}
					}
				}
			}

			if !self.all_targets {
				writeln!(stdout, "Note: These dependencies might be used by other targets.")?;
				if !self.lib
					&& !self.bins
					&& !self.examples
					&& !self.tests
					&& !self.benches
					&& self.bin.is_empty()
					&& self.example.is_empty()
					&& self.test.is_empty()
					&& self.bench.is_empty()
				{
					writeln!(stdout, "      To find dependencies that are not used by any target, enable `--all-targets`.")?;
				}
			}

			if dependency_names
				.values()
				.any(|d| !(d.normal_dev_non_lib.is_empty() && d.build_non_lib.is_empty()))
			{
				writeln!(stdout, "Note: Some dependencies are non-library packages.")?;
				writeln!(stdout, "      `cargo-udeps` regards them as unused.")?;
			}

			writeln!(
				stdout,
				"Note: They might be false-positive.\n      \
				 For example, `cargo-udeps` cannot detect usage of crates that are only used in doc-tests.",
			)?;
			stdout.flush()?;
			Ok(1)
		} else {
			writeln!(stdout, "All deps seem to have been used.")?;
			stdout.flush()?;
			Ok(0)
		}
	}
}

struct ExecData {
	cargo_exe :OsString,
	supports_color :bool,
	relevant_cmd_infos :Vec<CmdInfo>,
}

impl ExecData {
	fn new(config :&Config) -> CargoResult<Self> {
		// `$CARGO` should be present when `cargo-udeps` is executed as `cargo udeps ..` or `cargo run -- udeps ..`.
		let cargo_exe = env::var_os(cargo::CARGO_ENV)
			.map(Ok::<_, failure::Error>)
			.unwrap_or_else(|| {
				// Unless otherwise specified, `$CARGO` is set to `config.cargo_exe()` for compilation commands which points at `cargo-udeps`.
				let cargo_exe = config.cargo_exe()?;
				config.shell().warn(format!(
					"Couldn't find $CARGO environment variable. Setting it to {}",
					cargo_exe.display(),
				))?;
				config.shell().warn(
					"`cargo-udeps` currently does not support basic Cargo commands such as `build`",
				)?;
				Ok(cargo_exe.into())
			})?;
		Ok(Self {
			cargo_exe,
			supports_color :config.shell().supports_color(),
			relevant_cmd_infos : Vec::new(),
		})
	}
}

struct Exec {
	data :Arc<Mutex<ExecData>>,
}

impl Executor for Exec {
	fn exec(&self, mut cmd :ProcessBuilder, id :PackageId, target :&Target,
			mode :CompileMode, on_stdout_line :&mut dyn FnMut(&str) -> CargoResult<()>,
			on_stderr_line :&mut dyn FnMut(&str) -> CargoResult<()>) -> CargoResult<()> {

		let cmd_info = cmd_info(id, target.is_custom_build(), &cmd).unwrap_or_else(|e| {
			panic!("Couldn't obtain crate info {:?}: {:?}", id, e);
		});
		let is_path = id.source_id().is_path();
		{
			// TODO unwrap used
			let mut bt = self.data.lock().unwrap();

			// If the crate is not a library crate,
			// we are not interested in its information.
			if is_path {
				bt.relevant_cmd_infos.push(cmd_info.clone());
			}
			if (!cmd_info.cap_lints_allow) != is_path {
				on_stderr_line(&format!(
					"{} (!cap_lints_allow)={} differs from is_path={} for id={}",
					if bt.supports_color {
						Colour::Yellow.bold().paint("warning:").to_string()
					} else {
						"warning:".to_owned()
					},
					!cmd_info.cap_lints_allow,
					is_path,
					id,
				))?;
			}
			cmd.env(cargo::CARGO_ENV, &bt.cargo_exe);
		}
		if is_path {
			std::env::set_var("RUST_SAVE_ANALYSIS_CONFIG",
				r#"{ "reachable_only": true, "full_docs": false, "pub_only": false, "distro_crate": false, "signatures": false, "borrow_data": false }"#);
			cmd.arg("-Z").arg("save-analysis");
		}
		DefaultExecutor.exec(cmd, id, target, mode, on_stdout_line, on_stderr_line)?;
		Ok(())
	}
	fn force_rebuild(&self, unit :&Unit) -> bool {
		let source_id = (*unit).pkg.summary().source_id();
		source_id.is_path()
	}
}

#[derive(Clone, Debug)]
struct CmdInfo {
	pkg :PackageId,
	custom_build :bool,
	crate_name :String,
	crate_type :String,
	extra_filename :String,
	cap_lints_allow :bool,
	out_dir :String,
	externs :Vec<(String, String)>,
}

impl CmdInfo {
	fn get_save_analysis_path(&self) -> PathBuf {
		let maybe_lib = if self.crate_type.ends_with("lib") ||
				self.crate_type == "proc-macro" {
			"lib"
		} else {
			""
		};
		let filename = maybe_lib.to_owned() +
			&self.crate_name + &self.extra_filename + ".json";
		Path::new(&self.out_dir)
			.join("save-analysis")
			.join(filename)
	}
	fn get_save_analysis(&self, shell :&mut Shell) -> CargoResult<CrateSaveAnalysis> {
		let p = self.get_save_analysis_path();
		shell.print_ansi(
			format!(
				"{} Loading save analysis from {:?}\n",
				if shell.supports_color() {
					Colour::Cyan.bold().paint("info:").to_string()
				} else {
					"info:".to_owned()
				},
				p,
			)
			.as_ref(),
		)?;
		let f = std::fs::read_to_string(p)?;
		let res = serde_json::from_str(&f)?;
		Ok(res)
	}
}

fn cmd_info(id :PackageId, custom_build :bool, cmd :&ProcessBuilder) -> CargoResult<CmdInfo> {
	let mut args_iter = cmd.get_args().iter();
	let mut crate_name = None;
	let mut crate_type = None;
	let mut extra_filename = None;
	let mut cap_lints_allow = false;
	let mut out_dir = None;
	let mut externs = Vec::<(String, String)>::new();
	while let Some(v) = args_iter.next() {
		if v == "--extern" {
			let arg = args_iter.next()
				.map(|a| a.to_str().expect("non-utf8 paths not supported atm"))
				.map(|a| {
					let mut splitter = a.split("=");
					if let (Some(n), Some(p)) = (splitter.next(), splitter.next()) {
						(n.to_owned(), p.to_owned())
					} else {
						panic!("invalid format for extern arg: {}", a);
					}
				});
			if let Some(e) = arg {
				externs.push(e);
			}
		} else if v == "--crate-name" {
			if let Some(name) = args_iter.next() {
				crate_name = Some(name.to_str()
					.expect("non-utf8 crate names not supported")
					.to_owned());
			}
		} else if v == "--crate-type" {
			if let Some(ty) = args_iter.next() {
				crate_type = Some(ty.to_str()
					.expect("non-utf8 crate names not supported")
					.to_owned());
			}
		} else if v == "--cap-lints" {
			if let Some(c) = args_iter.next() {
				if c == "allow" {
					cap_lints_allow = true;
				}
			}
		} else if v == "--out-dir" {
			if let Some(d) = args_iter.next() {
				out_dir = Some(d.to_str()
					.expect("non-utf8 crate names not supported")
					.to_owned());
			}
		} else if v == "-C" {
			if let Some(arg) = args_iter.next() {
				let arg = arg.to_str().expect("non-utf8 args not supported atm");
				let mut splitter = arg.split("=");
				if let (Some(n), Some(p)) = (splitter.next(), splitter.next()) {
					if n == "extra-filename" {
						extra_filename = Some(p.to_owned());
					}
				}
			}
		}
	}
	let pkg = id;
	let crate_name = crate_name.ok_or_else(|| failure::err_msg("crate name needed"))?;
	let crate_type = crate_type.unwrap_or("bin".to_owned());
	let extra_filename = extra_filename.ok_or_else(|| failure::err_msg("extra-filename needed"))?;
	let out_dir = out_dir.ok_or_else(|| failure::err_msg("outdir needed"))?;

	Ok(CmdInfo {
		pkg,
		custom_build,
		crate_name,
		crate_type,
		extra_filename,
		cap_lints_allow,
		out_dir,
		externs,
	})
}

#[derive(Debug, Default)]
struct DependencyNames {
	normal_dev_by_extern_crate_name :HashMap<String, InternedString>,
	normal_dev_by_lib_true_snakecased_name :HashMap<String, HashSet<InternedString>>,
	normal_dev_non_lib :HashSet<InternedString>,
	build_by_extern_crate_name :HashMap<String, InternedString>,
	build_by_lib_true_snakecased_name :HashMap<String, HashSet<InternedString>>,
	build_non_lib :HashSet<InternedString>,
}

impl DependencyNames {
	fn new(
		from :&Package,
		packages :&HashMap<PackageId, &Package>,
		resolve :&Resolve,
		shell :&mut Shell,
	) -> CargoResult<Self> {
		fn ambiguous_names(
			names :&HashMap<String, HashSet<InternedString>>,
		) -> BTreeMap<InternedString, &str> {
			names
				.iter()
				.filter(|(_, v)| v.len() > 1)
				.flat_map(|(k, v)| v.iter().map(move |&v| (v, k.deref())))
				.collect()
		}

		let mut this = Self::default();

		let from = from.package_id();

		for (to_pkg, deps) in resolve.deps(from) {
			let to_pkg = packages.get(&to_pkg).unwrap_or_else(|| panic!("could not find `{}`", to_pkg));

			// Not all dependencies contain `lib` targets as it is OK to append non-library packages to `Cargo.toml`.
			// Their `bin` targets can be built with `cargo build --bins -p <SPEC>` and are available in build scripts.
			if let Some(to_lib) = to_pkg
				.targets()
				.iter()
				.find(|t| t.is_lib())
			{
				let extern_crate_name = resolve.extern_crate_name(from, to_pkg.package_id(), to_lib)?;
				let lib_true_snakecased_name = to_lib.name().replace('-', "_");

				for dep in deps {
					let (by_extern_crate_name, by_lib_true_snakecased_name) = if dep.is_build() {
						(
							&mut this.build_by_extern_crate_name,
							&mut this.build_by_lib_true_snakecased_name,
						)
					} else {
						(
							&mut this.normal_dev_by_extern_crate_name,
							&mut this.normal_dev_by_lib_true_snakecased_name,
						)
					};

					by_extern_crate_name.insert(extern_crate_name.clone(), dep.name_in_toml());

					// Two `Dependenc`ies with the same name point at the same `Package`.
					by_lib_true_snakecased_name
						.entry(lib_true_snakecased_name.clone())
						.or_insert_with(HashSet::new)
						.insert(dep.name_in_toml());
				}
			} else {
				for dep in deps {
					if dep.is_build() {
						&mut this.build_non_lib
					} else {
						&mut this.normal_dev_non_lib
					}
					.insert(dep.name_in_toml());
				}
			}
		}

		let ambiguous_normal_dev = ambiguous_names(&this.normal_dev_by_lib_true_snakecased_name);
		let ambiguous_build = ambiguous_names(&this.build_by_lib_true_snakecased_name);

		if !(ambiguous_normal_dev.is_empty() && ambiguous_build.is_empty()) {
			let mut msg = format!(
				"Currently `cargo-udeps` cannot distinguish multiple crates with the same `lib` name. This may cause false negative\n\
				 `{}`\n",
				from,
			);
			let (edge, joint) = if ambiguous_build.is_empty() {
				(' ', '└')
			} else {
				('│', '├')
			};
			for (ambiguous, edge, joint, prefix) in &[
				(ambiguous_normal_dev, edge, joint, "(dev-)"),
				(ambiguous_build, ' ', '└', "build-"),
			] {
				if !ambiguous.is_empty() {
					writeln!(msg, "{}─── {}dependencies", joint, prefix).unwrap();
					let mut ambiguous = ambiguous.iter().peekable();
					while let Some((dep, lib)) = ambiguous.next() {
						let joint = if ambiguous.peek().is_some() {
							'├'
						} else {
							'└'
						};
						writeln!(msg, "{}    {}─── {:?} → {:?}", edge, joint, dep, lib).unwrap();
					}
				}
			}
			shell.warn(msg.trim_end())?;
		}

		Ok(this)
	}
}
