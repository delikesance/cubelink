fn main() {
    // Manifeste Windows : compatibilite Windows 10/11 (indispensable aux fenetres enfants a transparence par pixel, utilisees par le
    // rideau de la zone de jeu integree) + Common Controls 6, que Tauri declare par defaut.
    let manifest = r#"
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
</assembly>
"#;
    // Ce manifeste n'est lié qu'aux exécutables (`rustc-link-arg-bins`), pas au binaire des tests unitaires de la lib.
    // Celui-ci référence les commandes (test des liaisons générées), donc `TaskDialogIndirect` de Common Controls 6 :
    // sans manifeste, Windows refuse de le charger (STATUS_ENTRYPOINT_NOT_FOUND). Chargement différé de comctl32 :
    // la fonction n'est résolue qu'au premier appel, que les tests ne font jamais ; l'application, elle, a le manifeste.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg=/DELAYLOAD:comctl32.dll");
        println!("cargo:rustc-link-arg=delayimp.lib");
    }
    let windows = tauri_build::WindowsAttributes::new().app_manifest(manifest);
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows)).expect("tauri build");
}
