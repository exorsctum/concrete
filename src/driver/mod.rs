use crate::ast::CompilationUnit;
use crate::ast::modules::ModuleDefItem;
use crate::compile_unit_info::{CompileUnitInfo, DebugInfo, OptLevel};
use crate::ir::lowering::lower_compile_units;
use crate::parser::ProgramSource;
use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use clap::Args;
use clap::{Parser, Subcommand};
use config::{Dependency, Package, Profile};
use git2::{IndexAddOption, Oid, Repository};
use owo_colors::OwoColorize;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::sync::Arc;
use std::{collections::HashMap, fs::File, path::PathBuf, time::Instant};
use tracing::debug;

use config::Config;
use linker::{link_binary, link_shared_lib};

pub mod config;
pub mod linker;

#[derive(Parser, Debug)]
#[command(author, version, about = "The Concrete Programming Language", long_about = None, bin_name = "concrete")]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Initialize a project
    New {
        path: PathBuf,

        /// The name of the project, defaults to the directory name
        #[arg(long)]
        name: Option<String>,

        /// Use a library template
        #[arg(long, group = "binary")]
        lib: bool,
    },
    /// Build a project or file
    Build(BuildArgs),
    /// Run a project or file
    Run(BuildArgs),
    /// Test a project or file.
    Test(BuildArgs),
}

#[derive(Args, Debug)]
pub struct BuildArgs {
    /// Build specific file
    #[arg(required = false)]
    path: Option<PathBuf>,

    /// Build the specific file as a library, only used when compiling a single file.
    #[arg(short, long, required = false, default_value_t = false)]
    lib: bool,

    /// Build for release with all optimizations.
    #[arg(short, long, default_value_t = false)]
    release: bool,

    /// Override the profile to use.
    #[arg(short, long)]
    profile: Option<String>,

    /// Also output the ast.
    #[arg(long, default_value_t = false)]
    ast: bool,

    /// Also output the ir.
    #[arg(long, default_value_t = false)]
    ir: bool,

    /// Also output the llvm ir file.
    #[arg(long, default_value_t = false)]
    llvm: bool,

    /// Also output the mlir file
    #[arg(long, default_value_t = false)]
    mlir: bool,

    /// Also output the asm file.
    #[arg(long, default_value_t = false)]
    asm: bool,

    /// Also output the object file.
    #[arg(long, default_value_t = false)]
    object: bool,

    /// This option is for checking the program for linearity.
    #[arg(long, default_value_t = false)]
    check: bool,
}

#[derive(Parser, Debug)]
#[command(author, version, about = "concrete compiler", long_about = None)]
pub struct CompilerArgs {
    /// The input file.
    input: PathBuf,

    /// The output file.
    pub output: PathBuf,

    /// Build for release with all optimizations.
    #[arg(short, long, default_value_t = false)]
    release: bool,

    /// Set the optimization level, 0,1,2,3
    #[arg(short = 'O', long)]
    optlevel: Option<u8>,

    /// Always add debug info
    #[arg(long)]
    pub debug_info: Option<bool>,

    /// Build as a library.
    #[arg(short, long, default_value_t = false)]
    library: bool,

    /// Also output the ast.
    #[arg(long, default_value_t = false)]
    ast: bool,

    /// Also output the ir.
    #[arg(long, default_value_t = false)]
    ir: bool,

    /// Also output the llvm ir file.
    #[arg(long, default_value_t = false)]
    llvm: bool,

    /// Also output the mlir file
    #[arg(long, default_value_t = false)]
    mlir: bool,

    /// Also output the asm file.
    #[arg(long, default_value_t = false)]
    asm: bool,

    /// Also output the object file.
    #[arg(long, default_value_t = false)]
    object: bool,

    /// This option is for checking the program for linearity.
    #[arg(long, default_value_t = false)]
    check: bool,
}

pub fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::New { path, name, lib } => {
            let name = name.unwrap_or_else(|| {
                path.file_name()
                    .context("failed to get project name")
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            });

            if !path.exists() {
                std::fs::create_dir_all(&path).context("failed to create the project directory")?;
                std::fs::create_dir_all(path.join("src")).context("failed to create src/")?;
            }

            let config_path = path.join("Concrete.toml");

            let mut profiles = HashMap::new();

            profiles.insert(
                "release".to_string(),
                Profile {
                    release: true,
                    opt_level: 3,
                    debug_info: false,
                },
            );

            profiles.insert(
                "dev".to_string(),
                Profile {
                    release: false,
                    opt_level: 0,
                    debug_info: true,
                },
            );

            let config = Config {
                package: Package {
                    name: name.clone(),
                    version: "0.1.0".to_string(),
                    license: "MIT".to_string(),
                },
                profile: profiles,
                dependencies: HashMap::new(),
            };

            std::fs::write(config_path, toml::to_string_pretty(&config)?)
                .context("failed to write Concrete.toml")?;
            std::fs::write(path.join(".gitignore"), "/build\n.stones/\n")
                .context("failed to write .gitignore")?;
            std::fs::write(
                path.join(".gitattributes"),
                "*.con linguist-language=Rust\n",
            )
            .context("failed to write .gitattributes")?;

            if !lib {
                std::fs::write(
                    path.join("src").join("main.con"),
                    format!(
                        r#"
        mod {} {{
            pub fn main() -> i32 {{
                return 0;
            }}
        }}"#,
                        name
                    ),
                )?;
            } else {
                std::fs::write(
                    path.join("src").join("lib.con"),
                    format!(
                        r#"
        mod {} {{
            pub fn hello_world() -> i32 {{
                return 0;
            }}
        }}"#,
                        name
                    ),
                )?;
            }

            {
                let repo = Repository::init(&path).context("failed to create repository")?;
                let sig = repo.signature()?;
                let tree_id = {
                    let mut index = repo.index()?;

                    index.add_all(["."].iter(), IndexAddOption::DEFAULT, None)?;
                    index.write()?;
                    index.write_tree()?
                };

                let tree = repo.find_tree(tree_id).context("failed to find git tree")?;
                repo.commit(Some("HEAD"), &sig, &sig, "Initial commit", &tree, &[])
                    .context("failed to create initial commit")?;
            }

            if !lib {
                println!(
                    "  {} binary (application) `{}` package",
                    "Created".green().bold(),
                    name
                );
            } else {
                println!("  {} library `{}` package", "Created".green(), name);
            }
        }
        Commands::Build(args) => {
            handle_build(args)?;
        }
        Commands::Run(args) => {
            let output = handle_build(args)?.0;
            println!();
            Err(std::process::Command::new(output).exec())?;
        }
        Commands::Test(mut args) => {
            args.lib = true;
            let (output, tests) = handle_build(args)?;
            println!();

            let tests = Arc::new(tests);

            println!("Running {} tests", tests.len());

            let mut passed = 0;

            if !tests.is_empty() {
                let lib = unsafe { libloading::Library::new(output).expect("failed to load") };

                for test in tests.iter() {
                    print!("test {} ... ", test.symbol);
                    let test_fn = unsafe {
                        lib.get::<unsafe extern "C" fn() -> i32>(test.mangled_symbol.as_bytes())
                    };

                    if test_fn.is_err() {
                        println!("{}", "err".red());
                        eprintln!("Symbol not found: {:?}", test_fn);
                        continue;
                    }

                    let test_fn = test_fn.unwrap();

                    let result = unsafe { (test_fn)() };

                    if result == 0 {
                        passed += 1;
                        println!("{}", "ok".green());
                    } else {
                        println!("{}", "err".red());
                    }
                }
            }

            println!();
            if !tests.is_empty() {
                println!(
                    "test result: {}. {} passed; {} failed; ({:.2}%)",
                    if passed == tests.len() {
                        "ok".green().to_string()
                    } else {
                        "err".red().to_string()
                    },
                    passed,
                    tests.len() - passed,
                    ((passed as f64 / tests.len() as f64) * 100.0).bold()
                );
            }

            return Ok(());
        }
    }

    Ok(())
}

#[derive(Debug, Clone)]
pub struct TestInfo {
    pub mangled_symbol: String,
    pub symbol: String,
}

fn handle_build(
    BuildArgs {
        path,
        release,
        profile,
        ast,
        ir,
        llvm,
        mlir,
        asm,
        object,
        lib,
        check,
    }: BuildArgs,
) -> Result<(PathBuf, Vec<TestInfo>)> {
    match path {
        // Single file compilation
        Some(input) => {
            let input_stem = input
                .file_stem()
                .context("could not get file stem")?
                .to_str()
                .context("could not convert file stem to string")?;

            let build_dir = std::env::current_dir()?;
            let output = build_dir.join(input_stem);

            let compile_args = CompilerArgs {
                input: input.clone(),
                output: output.clone(),
                release,
                optlevel: None,
                debug_info: None,
                library: lib,
                ast,
                ir,
                llvm,
                asm,
                object,
                mlir,
                check,
            };

            println!(
                "   {} {} ({})",
                "Compiling".green().bold(),
                input_stem,
                input.display()
            );

            let start = Instant::now();
            let ast_file = parse_file(input.clone())?;
            let (object, tests) = compile(&compile_args, &[ast_file])?;

            if lib {
                link_shared_lib(&[object.clone()], &output)?;
            } else {
                link_binary(&[object.clone()], &output)?;
            }

            if !compile_args.object {
                std::fs::remove_file(object)?;
            }

            let elapsed = start.elapsed();

            println!(
                "   {} {} in {elapsed:?}",
                "Finished".green().bold(),
                if release { "release" } else { "dev" },
            );

            Ok((output, tests))
        }
        // Project compilation.
        None => {
            let mut current_dir = std::env::current_dir()?;
            let mut config_path = None;
            for _ in 0..3 {
                if !current_dir.join("Concrete.toml").exists() {
                    current_dir = if let Some(parent) = current_dir.parent() {
                        parent.to_path_buf()
                    } else {
                        bail!("couldn't find Concrete.toml");
                    };
                } else {
                    config_path = Some(current_dir.join("Concrete.toml"));
                    break;
                }
            }
            let config_path = match config_path {
                Some(x) => x,
                None => bail!("couldn't find Concrete.toml"),
            };
            let base_dir = config_path
                .parent()
                .context("couldn't get config parent dir")?;
            let mut config = File::open(&config_path).context("failed to open Concrete.toml")?;
            let mut buf = String::new();
            config.read_to_string(&mut buf)?;
            let config: Config = toml::from_str(&buf).context("failed to parse Concrete.toml")?;
            let src_dir = base_dir.join("src");
            let target_dir = base_dir.join("build");
            if !target_dir.exists() {
                std::fs::create_dir_all(&target_dir)?;
            }
            let mut output = target_dir.join(config.package.name);
            let (profile, profile_name) = if let Some(profile) = profile {
                (
                    config
                        .profile
                        .get(&profile)
                        .context("couldn't get requested profile")?,
                    profile,
                )
            } else if release {
                (
                    config
                        .profile
                        .get("release")
                        .context("couldn't get profile: release")?,
                    "release".to_string(),
                )
            } else {
                (
                    config
                        .profile
                        .get("dev")
                        .context("couldn't get profile: dev")?,
                    "dev".to_string(),
                )
            };

            let lib_ed = src_dir.join("lib.con");
            let main_ed = src_dir.join("main.con");

            let start = Instant::now();

            let mut tests = Vec::new();

            let mut added_deps = HashMap::new();
            let compile_units_ast = compile_project(base_dir, false, &mut added_deps)?;

            for file in [main_ed, lib_ed] {
                if file.exists() {
                    let is_lib = file.file_stem().unwrap() == "lib";

                    let compile_args = CompilerArgs {
                        input: file,
                        output: if is_lib {
                            let name = output.file_stem().unwrap().to_string_lossy().to_string();
                            let name = format!("lib{name}");
                            output
                                .with_file_name(name)
                                .with_extension(CompileUnitInfo::get_platform_library_ext())
                        } else {
                            output.clone()
                        },
                        release,
                        optlevel: Some(profile.opt_level),
                        debug_info: Some(profile.debug_info),
                        library: is_lib,
                        ast,
                        ir,
                        llvm,
                        asm,
                        object,
                        mlir,
                        check,
                    };
                    let (object, file_tests) = compile(&compile_args, &compile_units_ast)?;
                    tests.extend(file_tests);

                    if compile_args.library {
                        link_shared_lib(&[object], &compile_args.output)?;
                    } else {
                        link_binary(&[object], &compile_args.output)?;
                    }

                    if is_lib {
                        output = compile_args.output;
                    }
                }
            }
            let elapsed = start.elapsed();
            println!(
                "   {} {} [{}{}] in {elapsed:?}",
                "Finished".green().bold(),
                profile_name,
                if profile.opt_level > 0 {
                    "optimized"
                } else {
                    "unoptimized"
                },
                if profile.debug_info {
                    " + debuginfo"
                } else {
                    ""
                }
            );

            Ok((output, tests))
        }
    }
}

pub fn compile_project(
    project_dir: &Path,
    is_dep: bool,
    added_deps: &mut HashMap<String, Dependency>,
) -> Result<Vec<CompilationUnit>> {
    let config_path = project_dir.join("Concrete.toml");
    let mut config = File::open(&config_path).context("failed to open Concrete.toml")?;
    let mut buf = String::new();
    config.read_to_string(&mut buf)?;
    let config: Config = toml::from_str(&buf).context("failed to parse Concrete.toml")?;

    let mut deps = Vec::new();

    for (name, info) in config.dependencies.iter() {
        if added_deps.contains_key(name) {
            // TODO: better dependency unification.
            // Maybe allow duplicate dependencies, however we can't allow duplicate stds due to lang items.
            continue;
        }

        let path = checkout_dependency(project_dir, name, info)?;

        added_deps.insert(name.clone(), info.clone());

        let compile_units = compile_project(&path, true, added_deps)?;

        deps.extend(compile_units);
    }

    println!(
        "   {} {} v{} ({})",
        "Compiling".green().bold(),
        config.package.name,
        config.package.version,
        project_dir.display()
    );

    let src_dir = project_dir.join("src");

    let lib_ed = src_dir.join("lib.con");
    let main_ed = src_dir.join("main.con");

    for file in [main_ed, lib_ed] {
        if file.exists() {
            let is_lib = file.file_stem().unwrap() == "lib";

            if !is_lib && is_dep {
                continue;
            }

            let compile_unit_ir = parse_file(file)?;

            deps.push(compile_unit_ir);
        }
    }

    Ok(deps)
}

pub fn checkout_dependency(base_dir: &Path, name: &str, dep: &Dependency) -> Result<PathBuf> {
    if let Some(path) = &dep.path {
        return Ok(path.clone());
    }

    if let Some(git) = &dep.git {
        let bricks_folder = base_dir.join(".bricks");

        if !bricks_folder.exists() {
            std::fs::create_dir_all(&bricks_folder)?;
        }

        let dir = bricks_folder.join(name);

        if dir.exists() {
            return Ok(dir);
        }

        println!(
            "   {} {} ({})",
            "Downloading".green().bold(),
            name,
            dep.r#ref.clone().unwrap_or("head".to_string()),
        );

        let repo = Repository::clone_recurse(git, &dir).context("Failed to clone dependency")?;

        if let Some(commit) = &dep.r#ref {
            let comm = repo.find_commit(Oid::from_str(commit)?)?;
            repo.checkout_tree(comm.as_object(), None)?;
        }

        Ok(dir)
    } else {
        anyhow::bail!("No path or git specified for dependency.")
    }
}

pub fn parse_file(mut path: PathBuf) -> Result<CompilationUnit> {
    if path.is_dir() {
        path = path.join("mod.ed");
    }

    let real_source = std::fs::read_to_string(&path)?;
    let source = ProgramSource::new(real_source.clone(), &path);

    let mut compile_unit = match crate::parser::parse_ast(&source) {
        Ok(x) => x,
        Err(diagnostic) => {
            diagnostic.render(&source);

            std::process::exit(1);
        }
    };

    let mut modules_to_add: HashMap<String, Vec<CompilationUnit>> = HashMap::new();
    for module in &compile_unit.modules {
        let mut list = Vec::new();
        for stmt in &module.contents {
            if let ModuleDefItem::ExternalModule(external_module) = stmt {
                let base_path = path.parent().unwrap();
                let mut module_path = base_path.join(&external_module.name).with_extension("con");

                debug!(
                    "Checking module {:?} at {}",
                    external_module.name,
                    module_path.display()
                );
                if !module_path.exists() {
                    module_path = base_path.join(&external_module.name).join("mod.con");
                }

                if !module_path.exists() {
                    bail!(
                        "External module '{}' not found at {}",
                        external_module.name,
                        module_path.display()
                    );
                }

                debug!(
                    "Parsing externally declared module '{}'",
                    module_path.display()
                );
                let parsed_unit = parse_file(module_path.clone())?;
                list.push(parsed_unit);
            }
        }
        modules_to_add.insert(module.name.name.clone(), list);
    }

    for (name, list) in modules_to_add.into_iter() {
        for module in &mut compile_unit.modules {
            if module.name.name == *name {
                for subunit in list.into_iter() {
                    for submodule in subunit.modules {
                        module
                            .contents
                            .push(ModuleDefItem::Module(submodule.into()));
                    }
                }
                break;
            }
        }
    }

    Ok(compile_unit)
}

pub fn compile(args: &CompilerArgs, ir: &[CompilationUnit]) -> Result<(PathBuf, Vec<TestInfo>)> {
    let start_time = Instant::now();

    let session = CompileUnitInfo {
        debug_info: if let Some(debug_info) = args.debug_info {
            if debug_info {
                DebugInfo::Full
            } else {
                DebugInfo::None
            }
        } else if args.release {
            DebugInfo::None
        } else {
            DebugInfo::Full
        },
        optlevel: if let Some(optlevel) = args.optlevel {
            match optlevel {
                0 => OptLevel::None,
                1 => OptLevel::Less,
                2 => OptLevel::Default,
                _ => OptLevel::Aggressive,
            }
        } else if args.release {
            OptLevel::Aggressive
        } else {
            OptLevel::None
        },
        library: args.library,
        output_file: args.output.with_extension("o"),
        output_asm: args.asm,
        output_ll: args.llvm,
        output_mlir: args.mlir,
    };
    tracing::debug!("Output file: {:#?}", session.output_file);
    tracing::debug!("Is library: {:#?}", session.library);
    tracing::debug!("Optlevel: {:#?}", session.optlevel);
    tracing::debug!("Debug Info: {:#?}", session.debug_info);

    if args.ast {
        std::fs::write(
            session.output_file.with_extension("ast"),
            format!("{:#?}", ir),
        )?;
    }

    let compile_unit_ir = match lower_compile_units(ir) {
        Ok(ir) => ir,
        Err(error) => {
            let report = crate::check::lowering_error_to_report(error);
            report.eprint(ariadne::FnCache::new(|x: &String| {
                std::fs::read_to_string(Path::new(x.as_str()))
            }))?;
            //report.eprint(ariadne::sources(path_cache))?;
            std::process::exit(1);
        }
    };

    if args.ir {
        std::fs::write(
            session.output_file.with_extension("ir"),
            format!("{:#?}", compile_unit_ir),
        )?;
    }

    let object_path = crate::codegen::compile(&session, &compile_unit_ir).unwrap();

    let elapsed = start_time.elapsed();
    tracing::debug!("Done in {:?}", elapsed);

    let mut test_names = Vec::new();
    for t in &compile_unit_ir.tests {
        let f = compile_unit_ir.functions[*t].as_ref().unwrap();
        test_names.push(TestInfo {
            mangled_symbol: f.name.clone(),
            symbol: f.debug_name.clone().unwrap(),
        });
    }

    Ok((object_path, test_names))
}
