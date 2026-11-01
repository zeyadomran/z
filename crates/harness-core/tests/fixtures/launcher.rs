use std::io::{Read, Write};

#[cfg(windows)]
#[path = "../support/launcher_native.rs"]
mod native;

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    #[cfg(windows)]
    if args[0] != "echo" {
        return windows_fixture(&args);
    }
    #[cfg(windows)]
    if let Ok(handle) = std::env::var("FIXTURE_UNRELATED_HANDLE") {
        native::try_signal_inherited(&handle);
    }
    assert_eq!(std::io::stdin().read(&mut [0]).unwrap(), 0);
    println!("{}", serde_json::to_string(&args[1..]).unwrap());
    println!("{}", std::env::current_dir().unwrap().display());
    std::io::stderr().write_all(&[0, 255, 1, 254]).unwrap();
}

#[cfg(windows)]
fn windows_fixture(args: &[String]) {
    use harness_core::execution::{Cancellation, LaunchSpec, Outcome, prepare};
    use std::{process::Command, time::Duration};
    let spec = |mode: &str| LaunchSpec {
        executable: std::env::current_exe().unwrap(),
        args: vec![mode.into()],
        cwd: std::env::current_dir().unwrap(),
        environment: vec![],
        timeout: Duration::from_secs(20),
        output_limit: 1024 * 1024,
    };
    match args[0].as_str() {
        "restricted" => {
            let _host = native::restricted_host_job();
            assert!(
                prepare(spec("echo")).is_err(),
                "incompatible job must fail closed"
            );
            println!("restricted rejected");
        }
        "nested" => {
            assert!(native::in_job());
            let result = prepare(spec("echo")).unwrap().run(&Cancellation::default());
            assert_eq!(result.outcome, Outcome::Completed);
            assert!(result.shutdown_confirmed);
            assert_eq!(result.root_exit_code, Some(0));
            println!("nested completed");
        }
        "boundary" => {
            let child = prepare(spec("echo")).unwrap();
            std::fs::write("child.pid", child.process_id().to_string()).unwrap();
            native::Event::open(&args[1]).set();
            native::Event::open(&args[2]).wait();
            drop(child);
        }
        "leaf" => {
            std::fs::write("leaf.pid", std::process::id().to_string()).unwrap();
            native::Event::open(&args[1]).set();
            native::Event::open(&args[2]).wait();
        }
        "tree" | "linger" => {
            let leaf_ready = native::Event::new();
            #[expect(
                clippy::zombie_processes,
                reason = "linger fixture deliberately exits first; integration driver retains the leaf handle and verifies job cleanup"
            )]
            let mut leaf = Command::new(std::env::current_exe().unwrap())
                .args([
                    "leaf",
                    &leaf_ready.name,
                    if args[0] == "linger" {
                        &args[3]
                    } else {
                        &args[2]
                    },
                ])
                .spawn()
                .unwrap();
            leaf_ready.wait();
            if args[0] == "tree" {
                write_streams();
            }
            native::Event::open(&args[1]).set();
            native::Event::open(&args[2]).wait();
            if args[0] == "tree" {
                leaf.wait().unwrap();
            } else {
                // Intentional normal root exit while the job still owns leaf.
                std::process::exit(0);
            }
        }
        "streams" => {
            write_streams();
            native::Event::open(&args[1]).set();
            native::Event::open(&args[2]).wait();
        }
        "idle" => {
            native::Event::open(&args[1]).set();
            native::Event::open(&args[2]).wait();
        }
        "limit" => {
            write_streams();
        }
        _ => panic!("unknown fixture"),
    }
}

#[cfg(windows)]
fn write_streams() {
    // Each write exceeds the pipe capacity; joining both writers proves neither
    // stream can deadlock behind a sequential read-to-EOF of the other stream.
    let out = std::thread::spawn(|| {
        std::io::stdout()
            .write_all(&vec![0xab; 256 * 1024])
            .unwrap()
    });
    let err = std::thread::spawn(|| {
        std::io::stderr()
            .write_all(&vec![0xcd; 256 * 1024])
            .unwrap()
    });
    out.join().unwrap();
    err.join().unwrap();
}
