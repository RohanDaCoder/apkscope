use super::*;

#[test]
fn frida_types() {
    assert_eq!(frida_type("I"), "int");
    assert_eq!(frida_type("Ljava/lang/String;"), "java.lang.String");
    assert_eq!(frida_type("[B"), "[B");
    assert_eq!(frida_type("[Ljava/lang/String;"), "[Ljava.lang.String;");
}

#[test]
fn readable_method_names() {
    let (c, sig, shown) = pretty("La/b/Outer$In;->run(I[BLjava/lang/String;)V");
    assert_eq!(
        (c.as_str(), sig.as_str(), shown.as_str()),
        ("Outer.In", "run(int, byte[], String)", "run")
    );
    assert_eq!(pretty("La/B;-><init>()V").1, "new B()");
    assert_eq!(call_needle("La/B;-><init>()V"), "new B(");
}

#[test]
fn base64_decodes() {
    let s: Vec<u16> = "aGk=".encode_utf16().collect();
    let b: Vec<i64> = base64(&s, 0)
        .unwrap()
        .iter()
        .map(|v| v.int().unwrap())
        .collect();
    assert_eq!(b, vec![104, 105]);
}

#[test]
fn mappings_round_trip() {
    let mut n = Notes::default();
    let jobf = "c a.b.C = Login\nm a.b.C.a(I)V = check\nf a.b.C.x:I = count\n";
    assert_eq!(import_mapping(jobf, &mut n), 3);
    assert_eq!(n.classes["La/b/C;"], "La/b/Login;");
    assert_eq!(n.members["La/b/C;->a()"], "check");
    assert_eq!(n.members["La/b/C;->x"], "count");
    let mut e = Notes::default();
    let enigma = "CLASS a/b/C com/x/Login\n\tCOMMENT main screen\n\tMETHOD a check (I)V\n\t\tCOMMENT validates\n\tFIELD x count I\n";
    assert_eq!(import_mapping(enigma, &mut e), 3);
    assert_eq!(e.classes["La/b/C;"], "Lcom/x/Login;");
    assert_eq!(e.members["La/b/C;->a()"], "check");
    assert_eq!(e.comments["La/b/C;->a()"], "validates");
    assert_eq!(e.comments["La/b/C;"], "main screen");
}

#[test]
fn mapping_format_from_file() {
    use MappingFormat as M;
    assert!(matches!(M::from_file("out.jobf"), M::Jobf));
    assert!(matches!(M::from_file("out.mapping"), M::Enigma));
    assert!(matches!(M::from_file("mapping.txt"), M::ProGuard));
    // the old default (no/other extension) stays Enigma
    assert!(matches!(M::from_file("out"), M::Enigma));
    assert!(matches!(M::from_file("out.pro"), M::Enigma));
}

#[test]
fn split_proto_javas_the_signature() {
    assert_eq!(
        split_proto("(ILjava/lang/String;)V"),
        ("int,java.lang.String".to_string(), "V".to_string())
    );
    assert_eq!(split_proto("()V"), (String::new(), "V".to_string()));
    assert_eq!(
        split_proto("(La/b/C;[B)I"),
        ("a.b.C,[B".to_string(), "I".to_string())
    );
}

#[test]
fn proguard_export_writes_the_headers() {
    // no dex needed for the class-header side; members only come from
    // dex lookups, so a notes-only export exercises headers + comments
    let mut n = Notes::default();
    n.classes
        .insert("La/b/C;".to_string(), "La/b/Login;".to_string());
    let o = export_mapping(&[], &n, MappingFormat::ProGuard);
    assert_eq!(o, "a.b.Login -> a.b.C:\n");
}

#[test]
fn proguard_import_detects_the_arrow_format() {
    // a mapping.txt routed through import_mapping, not proguard::parse,
    // proves the detection: `->` and no ` = ` like a .jobf line
    let mut n = Notes::default();
    let proguard = concat!(
        "# R8\n",
        "com.example.app.MainActivity -> a.b:\n",
        "    int status -> a\n",
        "    void onCreate(android.os.Bundle) -> a\n",
    );
    assert_eq!(import_mapping(proguard, &mut n), 3);
    assert_eq!(n.classes["La/b;"], "Lcom/example/app/MainActivity;");
    assert_eq!(n.members["La/b;->a"], "status");
    assert_eq!(n.members["La/b;->a()"], "onCreate");
    // and a .jobf line still routes to the jobf branch: ` = `, no `->`
    let mut j = Notes::default();
    let jobf = "c a.b.C = Login\n";
    assert_eq!(import_mapping(jobf, &mut j), 1);
    assert_eq!(j.classes["La/b/C;"], "La/b/Login;");
}

#[test]
fn evaluates_xor_decryptor() {
    // static String d(String s): char[] c = s.toCharArray(); for (i..) c[i] ^= 1; return new String(c);
    // run directly on the VM with a hand-built method body
    let d = crate::dex::tests::tiny_dex();
    let dx = Dex::parse(&d).unwrap();
    let dexes = [dx];
    let mut vm = Vm {
        dexes: &dexes,
        steps: 0,
        statics: HashMap::new(),
        inited: HashSet::new(),
    };
    // registers v0..v3, arg in v3 (a char[]): v0 = len; v1 = 0; loop: if v1 >= v0 goto end;
    // v2 = v3[v1]; v2 ^= 1; v2 = (char) v2; v3[v1] = v2; v1++; goto loop; end: return-object v3
    let units: [u16; 13] = [
        0x3021, 0x0112, 0x0135, 0x000b, 0x0249, 0x0103, 0x02df, 0x0102, 0x0250, 0x0103, 0x01d8,
        0x0101, 0xf628,
    ];
    let mut code_units = units.to_vec();
    code_units.push(0x0311); // return-object v3
    let bytes: Vec<u8> = code_units.iter().flat_map(|u| u.to_le_bytes()).collect();
    let code = dex::Code {
        registers: 4,
        ins: 1,
        insns: &bytes,
        tries: vec![],
    };
    let arr = V::Arr(Rc::new(RefCell::new(
        "ih".encode_utf16().map(|c| V::Int(c as i64)).collect(),
    )));
    let Some(Some(V::Arr(out))) = vm.run(&dexes[0], &code, vec![arr], 0, false) else {
        panic!("no result")
    };
    let s: String = out
        .borrow()
        .iter()
        .map(|v| v.int().unwrap() as u8 as char)
        .collect();
    assert_eq!(s, "hi");
}
