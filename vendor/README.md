# vendor/serenity

Serenity 0.12.5 from crates.io with one patch, marked `ORBIT-BOT PATCH` in
`src/model/application/component.rs`: `ActionRow` keeps the `component` of a
Label (type 18), so the bot can read text inputs and dropdowns in modal
submissions. Examples were removed. Drop this folder and the `[patch.crates-io]`
entry in `Cargo.toml` once serenity supports Labels.
