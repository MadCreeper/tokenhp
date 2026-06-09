import Foundation
import CoreText

/// Registers TTF fonts shipped in the kit's `Resources/` bundle with the
/// process so SwiftUI can resolve `.font(.custom(...))` by PostScript name.
/// Call once at app startup — subsequent calls are no-ops.
public enum FontRegistry {
    nonisolated(unsafe) private static var registered = false
    private static let lock = NSLock()

    /// PostScript name of the bundled pixel font (Monocraft, SIL OFL 1.1 —
    /// the Minecraft theme's typeface). See THIRD_PARTY_NOTICES.md.
    public static let pixelFontName = "Monocraft"

    public static func registerBundledFonts() {
        lock.lock(); defer { lock.unlock() }
        guard !registered else { return }
        registered = true

        for resource in ["Monocraft"] {
            guard let url = Bundle.module.url(forResource: resource, withExtension: "ttf") else {
                NSLog("HPBar: bundled font missing — %@", resource)
                continue
            }
            var err: Unmanaged<CFError>?
            if !CTFontManagerRegisterFontsForURL(url as CFURL, .process, &err) {
                if let e = err?.takeRetainedValue() {
                    NSLog("HPBar: failed to register font %@: %@", resource, String(describing: e))
                }
            }
        }
    }
}
