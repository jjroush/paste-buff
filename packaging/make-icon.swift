// Draws the paste-buff app icon and writes an .iconset for iconutil.
//
//   swift packaging/make-icon.swift packaging/AppIcon.iconset
//   iconutil -c icns packaging/AppIcon.iconset -o packaging/AppIcon.icns
//
// Layout follows the macOS icon grid: a 1024pt canvas with an 824pt
// rounded-square body inset 100pt, so it sits right next to system icons.

import AppKit

let out = CommandLine.arguments.dropFirst().first ?? "AppIcon.iconset"
try? FileManager.default.createDirectory(atPath: out, withIntermediateDirectories: true)

func draw(_ ctx: CGContext) {
    let body = CGRect(x: 100, y: 100, width: 824, height: 824)
    let squircle = CGPath(roundedRect: body, cornerWidth: 185, cornerHeight: 185, transform: nil)

    // Drop shadow under the body.
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -12), blur: 28,
                  color: CGColor(gray: 0, alpha: 0.35))
    ctx.addPath(squircle)
    ctx.setFillColor(CGColor(gray: 0, alpha: 1))
    ctx.fillPath()
    ctx.restoreGState()

    // Body: the landing page's blue → violet sweep.
    ctx.saveGState()
    ctx.addPath(squircle)
    ctx.clip()
    let space = CGColorSpace(name: CGColorSpace.sRGB)!
    let gradient = CGGradient(colorsSpace: space, colors: [
        CGColor(srgbRed: 0.04, green: 0.56, blue: 0.97, alpha: 1),
        CGColor(srgbRed: 0.43, green: 0.36, blue: 0.96, alpha: 1),
        CGColor(srgbRed: 0.73, green: 0.38, blue: 0.99, alpha: 1),
    ] as CFArray, locations: [0, 0.55, 1])!
    ctx.drawLinearGradient(gradient, start: CGPoint(x: 100, y: 924),
                           end: CGPoint(x: 924, y: 100), options: [])
    // Soft top highlight.
    let shine = CGGradient(colorsSpace: space, colors: [
        CGColor(gray: 1, alpha: 0.22), CGColor(gray: 1, alpha: 0),
    ] as CFArray, locations: [0, 1])!
    ctx.drawLinearGradient(shine, start: CGPoint(x: 512, y: 924),
                           end: CGPoint(x: 512, y: 560), options: [])
    ctx.restoreGState()

    // A stack of two cards: the clip behind, the current one in front.
    let back = CGRect(x: 318, y: 300, width: 388, height: 470)
    ctx.addPath(CGPath(roundedRect: back.offsetBy(dx: 36, dy: 36),
                       cornerWidth: 56, cornerHeight: 56, transform: nil))
    ctx.setFillColor(CGColor(gray: 1, alpha: 0.35))
    ctx.fillPath()

    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -8), blur: 24,
                  color: CGColor(gray: 0, alpha: 0.25))
    ctx.addPath(CGPath(roundedRect: back, cornerWidth: 56, cornerHeight: 56, transform: nil))
    ctx.setFillColor(CGColor(gray: 1, alpha: 1))
    ctx.fillPath()
    ctx.restoreGState()

    // Clip at the top of the front card.
    let clip = CGRect(x: 432, y: 728, width: 160, height: 84)
    ctx.addPath(CGPath(roundedRect: clip, cornerWidth: 30, cornerHeight: 30, transform: nil))
    ctx.setFillColor(CGColor(srgbRed: 0.30, green: 0.32, blue: 0.82, alpha: 1))
    ctx.fillPath()

    // Text lines, tinted with the body colours.
    let lines: [(CGFloat, CGFloat)] = [(640, 280), (568, 300), (496, 220), (424, 260)]
    for (i, (y, w)) in lines.enumerated() {
        let t = CGFloat(i) / CGFloat(lines.count - 1)
        ctx.addPath(CGPath(roundedRect: CGRect(x: 370, y: y, width: w, height: 34),
                           cornerWidth: 17, cornerHeight: 17, transform: nil))
        ctx.setFillColor(CGColor(srgbRed: 0.10 + 0.5 * t, green: 0.50 - 0.12 * t,
                                 blue: 0.96, alpha: 0.9))
        ctx.fillPath()
    }
}

func render(px: Int) -> Data {
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: px, pixelsHigh: px,
                               bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true,
                               isPlanar: false, colorSpaceName: .deviceRGB,
                               bytesPerRow: 0, bitsPerPixel: 0)!
    let gc = NSGraphicsContext(bitmapImageRep: rep)!
    let ctx = gc.cgContext
    ctx.scaleBy(x: CGFloat(px) / 1024, y: CGFloat(px) / 1024)
    draw(ctx)
    gc.flushGraphics()
    return rep.representation(using: .png, properties: [:])!
}

for size in [16, 32, 128, 256, 512] {
    for scale in [1, 2] {
        let name = scale == 1 ? "icon_\(size)x\(size).png" : "icon_\(size)x\(size)@2x.png"
        try! render(px: size * scale).write(to: URL(fileURLWithPath: "\(out)/\(name)"))
    }
}
