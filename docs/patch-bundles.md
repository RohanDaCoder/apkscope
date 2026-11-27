# Patch bundles

One XML file that patches an APK: it names the app it is for, and the patches it holds.
`apkscope patch` checks the app, applies the chosen patches, rebuilds and signs — no apktool,
smali.jar or apksigner needed.

```
apkscope patch unlock.xml app.apk out.apk
apkscope patch unlock.xml app.apk out.apk --only "Unlock" --ks my.jks --ks-pass pass:x
apkscope patch unlock.xml --list
```

The same bundle works over MCP (`patch_apk`) and from scripts.

## Why XML

A bundle names the app it targets and describes each edit as what it means — stub this
method, swap this text, set this string — instead of patching raw bytes. Older patch formats
work against DEX instruction bytes with wildcards masking the operands that move between
builds: they are written against exact bytes, in a format only one tool reads, and a pattern
that does not match anymore fails silently or worse.

A bundle keeps the ideas that made those patches useful and moves them up a level:

| the idea | a bundle |
| --- | --- |
| the target app | `<target package versionName minVersionCode maxVersionCode>`, checked against the manifest before anything runs |
| named patches picked from a list | `<patch name=...>`, picked with `--only` / `--skip`, shown by `--list` |
| swapping code for code | `<replace class method from to>` on the class's own smali text |
| stubbing a method to return a constant | `<return class method sig value>`, typed by the method's return type |
| resource / file edits bundled in | `<string>`, `<manifest>`, `<entry>` |

The smali text edits work because apkscope already round-trips a class through
its own smali (what `apkscope smali` writes, `build --smali` assembles): a bundle edits that
text and re-assembles the class, so every edit goes through the same assembler a person
editing by hand uses. A pattern that does not match is an error, not a silently unpatched
method.

## The format

```xml
<?xml version="1.0" encoding="utf-8"?>
<bundle name="Unlock" author="rohan" version="3" description="what it does">
  <target package="com.example.app" versionName="2.1" minVersionCode="21" maxVersionCode="29"/>
  <patch name="Unlock" description="license check off">
    <return class="com.example.app.License" method="isPurchased" sig="()Z" value="true"/>
    <replace class="com.example.app.AdLoader" method="load"
             from="const-string v0, &quot;ca-app-pub-394&quot;"
             to="const-string v0, &quot;0&quot;"/>
  </patch>
  <patch name="Strings">
    <string name="app_name">Patched app</string>
  </patch>
</bundle>
```

Everything about the format:

- **`<bundle>`** — `name`, `author`, `version`, `description`, all optional. Must be the root.
- **`<target>`** — what the bundle is for: `package` (required), and any of `versionName`,
  `minVersionCode`, `maxVersionCode`. Every part given must match the APK's manifest; a
  missing part of the manifest fails the check too. Omit `<target>` to patch any APK.
- **`<patch name=...>`** — a named group of ops, the checkbox. `description` optional; names
  are unique (two of one name is an error). Its ops run in file order.
- **`<return class method [sig] [value]/>`** — replace the method's whole body with a stub
  that returns a constant. `class` is a Java name or a descriptor (`com.example.app.License`
  or `Lcom/example/app/License;`), `sig` is the exact `name(params)ret`; without `sig` every
  method of that name is stubbed, overloads included. `value` is typed by the method's own
  return type: `true` / `false` for `Z`, a number or `0x` hex for `B` `S` `C` `I` `J` (a
  decimal or hex-bits float for `F` `D`), `null` for references; absent means the type's zero.
  A value the type cannot hold is an error, and so is a constructor (`<init>`): its stub
  drops the `super()` call and Android refuses the class — `<replace>` inside it instead.
  `abstract` and `native` methods have no body to edit and are errors too.
- **`<replace class method [sig] from to [once]/>`** — swap `from` for `to` in the bodies of
  the matching methods, every occurrence or only the first in each method (`once="yes"`).
  `from` and `to` are text as it appears in the class's smali, and must match within one line
  (smali is one instruction per line) — take the exact text from
  `apkscope smali app.apk a.b.C`. A `from` no method has is an error.
- **`<string name [config]>text</string>`** — set one string resource, like
  `build --string name[@config]=value`. The text is the element's content.
- **`<manifest file=.../>`** — replace `AndroidManifest.xml` with this text-XML file, like
  `build --manifest`. The path is relative to the bundle's own folder.
- **`<entry name [src|delete]/>`** — add or replace a zip entry with a file (`src`, relative
  to the bundle's folder), or delete the entry (`delete="yes"`).

Class and method names in a bundle are the names in that smali text — the real ones in the
DEX. The query commands (`methods`, `returns`, `decompile`) and the viewer show deobfuscated
names for methods R8 renamed, which differ: check the spelling with `apkscope smali` first.

File paths are resolved against the folder the bundle is in; absolute paths stay as they are.
XML entities (`&quot;`, `&amp;`, ...) work in attribute values and text. Unknown elements and
attributes are errors that name their line, not no-ops — a typo'd bundle is refused, not half
applied.

The whole bundle is applied before anything is written: a class that is not there, or a
`from` that no method has, fails with nothing written.

## What it does not do (yet)

- No raw DEX hex patterns, the `"original bytes"` / `"replaced bytes"` form: smali text is the
  edit surface, so a bundle is tied to a class's code shape but not to its exact bytes.
- No new instructions of your own: `<return>` and `<replace>` change what is there. Writing
  new method bodies is what `apkscope smali` + `build --smali` are for.
- Viewer surface: `patch` is CLI and MCP only in this experiment; the viewer's build window
  does not apply bundles yet.
- `<target>` `versionName` / `versionCode` are exact matches on what the manifest says; a
  bundle cannot follow an app across versions by itself.

Examples: `assets/demo.xml` exercises every op.
