//! Writes sample emails in every palette to a directory, for eyeballing the
//! design: `cargo run -p mitch-lib --example email_samples -- <out-dir>`.

use mitch_lib::email_theme as et;

fn main() -> std::io::Result<()> {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "email-samples".into());
    std::fs::create_dir_all(&out)?;
    let (primary, alt) = ("https://mitch.pro", "https://mitchdog.com");
    let unsub = "https://mitch.pro/unsubscribe/example";
    let footer = |p: &et::Palette| et::footer(p, primary, alt, Some(unsub), true);
    for name in [
        "default",
        "halloween",
        "thanksgiving",
        "christmas",
        "newyear",
        "valentines",
    ] {
        let p = et::palette_named(name);
        let code = format!(
            "{}{}{}{}",
            et::heading(&p, "Confirm your email"),
            et::paragraph(
                &p,
                "Your verification code is below. It expires in 10 minutes."
            ),
            et::code_box(&p, "482 916"),
            et::fine_print(
                &p,
                "If you didn't request this code, you can safely ignore this email."
            ),
        );
        let invite = format!(
            "{}{}{}{}",
            et::heading(&p, "alex invited you to mitch.pro"),
            et::paragraph(&p, "mitch.pro is a student-only site with games, tools, secure messaging and MitchCoins."),
            et::panel(
                &p,
                &format!("<p style=\"margin:0;font-size:15px;font-weight:700;color:{};\">Sign up and you both get 2,000 MitchCoins.</p>", p.ink),
            ),
            et::button(&p, "https://mitchdog.com", "Accept invite"),
        );
        let reply = format!(
            "{}{}",
            et::paragraph(&p, "Hi Sam,"),
            et::paragraph(&p, "Thanks for writing in. I reset your account, so you can sign in again with your school email. Let me know if anything still looks off."),
        );
        for (file, subject, body) in [
            ("code", "Confirm your email", code),
            ("invite", "You're invited to mitch.pro!", invite),
            ("reply", "Re: Can't sign in", reply),
        ] {
            let html = et::shell(&p, subject, "", &body, &footer(&p), "mitch.pro", alt);
            std::fs::write(format!("{out}/{name}-{file}.html"), html)?;
        }
    }
    Ok(())
}
