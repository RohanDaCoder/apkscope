//! Examples shown under each command's `--help`, and the recipes under `apkscope --help`.
//! Written for people and for AI agents: each line is a complete command that works as is.

pub const TOP: &str = "\
Recipes (Class = a.b.C, member = a.b.C#field or a.b.C#method()):
  what is this app            apkscope info app.apk;  apkscope scan app.apk;  apkscope size app.apk
  where is this string used   apkscope refs app.apk string \"api/v1/login\"
  who implements X            apkscope refs app.apk impl com.example.Proto
  read a class                apkscope decompile app.apk com.example.Main
  many classes at once        apkscope decompile app.apk --classes-file list.txt --out-dir src/
  the app's HTTP API          apkscope search app.apk api
  message ids / opcodes       apkscope returns app.apk uri --impl com.example.IProtocol
  decode a number             apkscope consts app.apk --value 405
  search one package          apkscope search app.apk code \"token\" --in com.example.net.*
  patch and rebuild           apkscope smali app.apk a.b.C > C.smali; (edit); apkscope build app.apk out.apk --smali C.smali
  sign with a keystore        apkscope build app.apk out.apk --ks my.jks --ks-pass pass:secret
  check a patch landed        apkscope diff app.apk out.apk --strings

For scripts and agents: -q drops progress lines from stderr, --json gives JSON where a command
lists items, --limit/--offset page long lists. Exit code 0 = ran fine (even with no results),
1 = error, 2 = bad usage. Every command is also an MCP tool: apkscope mcp.
More: docs/agents.md";

pub const CLASSES: &str = "\
Examples:
  apkscope classes app.apk --prefix com.example
  apkscope classes app.apk --app-only --json";

pub const METHODS: &str = "\
Examples:
  apkscope methods app.apk --class com.example.Main
  apkscope methods app.apk --filter login";

pub const STRINGS: &str = "\
Examples:
  apkscope strings app.apk --filter https://
Tip: to see which classes use a string, use `apkscope refs app.apk string <text>`.";

pub const REFS: &str = "\
Examples:
  apkscope refs app.apk string \"api/v1/login\"           every class that uses this string
  apkscope refs app.apk method com.example.Api#login     who calls it
  apkscope refs app.apk field com.example.Cfg#TOKEN      who reads / writes it
  apkscope refs app.apk type okhttp3.OkHttpClient        who mentions the type
  apkscope refs app.apk impl com.example.IProtocol       direct implementors
  apkscope refs app.apk extends android.app.Activity
  apkscope refs app.apk string token --regex --ignore-case --java   with the decompiled lines";

pub const DECOMPILE: &str = "\
Examples:
  apkscope decompile app.apk com.example.Main
  apkscope decompile app.apk com.example.Main --method onCreate
  apkscope decompile app.apk a.b.C a.b.D --out-dir src/        one file per class
  apkscope decompile app.apk --classes-file list.txt --out-dir src/
  apkscope decompile app.apk com.example.Main --smali
Class names work in the shown (deobfuscated) form and in the original obfuscated form.";

pub const DIFF: &str = "\
Examples:
  apkscope diff old.apk new.apk                 classes, methods, permissions, changed constants
  apkscope diff old.apk new.apk --strings       also added / removed strings (check that a patch landed)";

pub const CONSTS: &str = "\
Examples:
  apkscope consts app.apk com.example.Codes            every constant of one class
  apkscope consts app.apk com.example.net.*            of a package
  apkscope consts app.apk --value 405                  which constants are 405
  apkscope consts app.apk --name 'result|error' com.example.*
Static final fields with a value stored in the DEX (int, long, String, ...).";

pub const RETURNS: &str = "\
Examples:
  apkscope returns app.apk uri --impl com.example.IProtocol     every message class and its id
  apkscope returns app.apk uri --value 736131                   which class has this id
  apkscope returns app.apk getType --in com.example.msg.*
Finds methods whose whole body is `return <constant>;` (ids, opcodes, type codes).";

pub const SIZE: &str = "\
Examples:
  apkscope size app.apk
  apkscope size app.apk --top 50 --json
Read from the zip directory; duplicates are files with the same CRC and size.";

pub const SEARCH: &str = "\
Scopes:
  code      decompiled Java            string   string literals         usages  xref by name
  class     class names                res      resource XML + manifest (file:line)
  urls      endpoints                  http     HTTP client calls       secrets keys and tokens
  api       Retrofit endpoint table    config   BuildConfig, remote-config defaults, prefs reads
Examples:
  apkscope search app.apk code \"getToken\"
  apkscope search app.apk code 736131 --in com.example.room.*     only one package
  apkscope search app.apk code marshall --impl com.example.IProtocol
  apkscope search app.apk res \"@id/login_button\"                  which layout uses it
  apkscope search app.apk urls --all --format json
  apkscope search app.apk api
urls / http / secrets show app code only by default (--all adds libraries).";

pub const INFO: &str = "\
Examples:
  apkscope info app.apk     package, versions, SDKs, permissions, exported components, native libs";

pub const RESOURCE: &str = "\
Examples:
  apkscope resource app.apk res/layout/activity_main.xml
  apkscope resource app.apk AndroidManifest.xml";

pub const RECON: &str = "\
Examples:
  apkscope callers app.apk com.example.Api login
  apkscope access app.apk com.example.Session token";

pub const CALLGRAPH: &str = "\
Examples:
  apkscope callgraph app.apk com.example.Api login --depth 4";

pub const FRIDA: &str = "\
Examples:
  apkscope frida app.apk com.example.Api login > hook.js; frida -U -f com.example -l hook.js
  apkscope frida app.apk com.example.Api          every method of the class";

pub const INSTALL: &str = "Examples:
  apkscope install app.xapk                 base + the ABI split the device needs + density/language splits
  apkscope install app.apk -D emulator-5554
  apkscope install app.xapk --abi arm64-v8a   force an ABI (an arm64-only app on an x86_64 emulator)
It prints the primaryCpuAbi the package manager chose.";

pub const NOTE: &str = "\
Examples:
  apkscope rename app.apk a.b.c com.example.LoginActivity
  apkscope rename app.apk a.b.c#d() checkPassword
  apkscope comment app.apk a.b.c#d() \"returns true when the PIN matches\"
Saved in <apk>.apkscope.json next to the APK; decompile, search and the viewer use the names.";

pub const SMALI: &str = "\
Examples:
  apkscope smali app.apk com.example.Main > Main.smali
  (edit Main.smali)
  apkscope build app.apk out.apk --smali Main.smali";

pub const RES: &str = "\
Examples:
  apkscope res app.apk decoded/
  (edit decoded/res/values/strings.xml or decoded/AndroidManifest.xml)
  apkscope build app.apk out.apk --res decoded/
Also takes a folder decoded by apktool or APKEditor: changed and new files are applied,
nothing is removed.";

pub const BUILD: &str = "\
Examples:
  apkscope build app.apk out.apk --smali Main.smali
  apkscope build app.apk out.apk --res decoded/ --string app_name=Patched
  apkscope build app.apk out.apk --put classes7.dex=my.dex --delete META-INF/old.SF
  apkscope build app.apk out.apk --ks release.jks --ks-pass pass:secret --ks-alias key0
  apkscope build app.apk out.apk --ks release.p12 --ks-pass env:KS_PASS
  apkscope build app.apk out.apk --key k.pk8 --cert c.crt
Without a key it signs with apkscope's own debug key. --ks-pass takes pass:TEXT, env:VAR or
file:PATH. Keystores: JKS and PKCS#12 (.jks, .keystore, .p12, .pfx).
Then: adb install -r out.apk";

pub const VERIFY: &str = "\
Examples:
  apkscope verify out.apk     signature schemes and signer certificate";

pub const EXPORT: &str = "\
Examples:
  apkscope export app.apk project/      a Gradle project of the whole app";

pub const DISASM: &str = "\
Examples:
  apkscope disasm app.apk com.example.Main";
