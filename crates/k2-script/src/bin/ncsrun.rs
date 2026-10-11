//! ncsrun NAME [--module M] [--install DIR]: run a script against a stub
//! host that logs every engine call and returns default values.

use std::process::ExitCode;

use k2_res::{Install, Resources};
use k2_script::{load_ncs, load_routines, StubHost, Vm};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let mut name = None;
    let mut module = None;
    let mut install = std::env::var("K2_INSTALL").unwrap_or_else(|_| "..".into());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--module" => module = it.next().cloned(),
            "--install" => install = it.next().cloned().ok_or("--install needs a dir")?,
            _ => name = Some(a.trim_end_matches(".ncs").to_owned()),
        }
    }
    let name = name.ok_or("usage: ncsrun NAME [--module M] [--install DIR]")?;
    let mut res = Resources::open(Install::discover(&install)?)?;
    if let Some(m) = &module {
        res.load_module(m)?;
    }
    let routines = load_routines(&res)?;
    let ncs = load_ncs(&res, &name)?;
    let vm = Vm::new(&routines);
    let mut host = StubHost { log: true, ..Default::default() };
    println!("run {name}:");
    let out = vm.run(&ncs, &mut host, 0x100)?;
    println!("=> {:?} after {} steps", out.ret, out.steps);
    // Drain deferred actions (DelayCommand etc.) one level deep at a time.
    let mut rounds = 0;
    while !host.deferred.is_empty() && rounds < 8 {
        rounds += 1;
        for state in std::mem::take(&mut host.deferred) {
            println!("deferred @{:#x}:", state.offset);
            let o = vm.resume(&state, &mut host)?;
            println!("=> done after {} steps", o.steps);
        }
    }
    Ok(())
}
