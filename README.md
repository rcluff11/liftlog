# Liftlog

This is a command-line lifting tracker I built in Rust to learn the language. You log your sets from the terminal, and it saves them to a SQLite database so you can look back at your history, see your PRs, and get a suggestion for what weight and reps to try next session. I wanted something that actually helps me get better at lifting instead of just storing numbers.

It estimates your 1RM for each set with the Epley formula (`weight × (1 + reps/30)`), so you can see your strength going up even when you aren't testing maxes. The `next` command uses double progression: each lift has a 3–6 rep range, and once you hit the top of the range on every set it tells you to go up in weight. If you didn't, it tells you to stay at the same weight and beat your weakest set.

Some of the Rust stuff I learned while building it:

* **Structs and enums**: the `Set` struct for weight and reps, and a `Command` enum for each subcommand
* **Result, Option, and the `?` operator**: for handling bad input and database errors without crashing
* **Iterators**: `.map()`, `.filter()`, and `.collect()` instead of writing loops by hand
* **Crates**: `clap` for parsing commands, `rusqlite` for the database, and `chrono` for dates

## Instructions for Build and Use

Steps to build and/or run the software:

1. Install Rust with rustup if you don't have it already.
2. Clone the repository and open the project folder in VS Code.
3. Run `cargo build` to download the crates and compile the project.
4. Run commands with `cargo run -- <command>`. The `--` has to come right after `run`.

Instructions for using the software:

1. Log your sets for a lift, written as weight x reps:

   ```
   cargo run -- log bench 275x6 275x6 275x6
   ```

   Add `--date 2026-11-05` to log a workout from a different day.

2. Use the other commands to check your progress:

   | Command | What it does |
   |---------|--------------|
   | `log <exercise> <sets>` | Save your sets for a lift |
   | `history <exercise>` | Show your past sessions for a lift |
   | `prs` | Show your best estimated 1RM for each lift |
   | `next <exercise>` | Suggest the weight and reps for next session |

3. The database file `liftlog.db` is created automatically the first time you log something. Delete it if you want to start over.

## Development Environment

To recreate the development environment, you need the following software and/or libraries with the specified versions:

* Rust (stable) and Cargo, installed through rustup
* clap (with the `derive` feature)
* rusqlite
* chrono
* VS Code with the rust-analyzer extension

## Useful Websites to Learn More

I found these websites useful in developing this software:

* [The Rust Programming Language Book](https://doc.rust-lang.org/book/)
* [Rust by Example](https://doc.rust-lang.org/rust-by-example/)
* [clap Documentation](https://docs.rs/clap/latest/clap/)
* [rusqlite Documentation](https://docs.rs/rusqlite/latest/rusqlite/)

##