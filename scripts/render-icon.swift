// Renders an SVG with a transparent background using AppKit, to a PNG or,
// when the output name ends in .rgba, to raw 8-bit RGBA pixels (row-major,
// top-left first) for embedding in the binary.
// Usage: swift scripts/render-icon.swift in.svg out.{png|rgba} size
import AppKit

let args = CommandLine.arguments
guard args.count == 4, let size = Int(args[3]) else {
    FileHandle.standardError.write("usage: render-icon.swift in.svg out.png size\n".data(using: .utf8)!)
    exit(2)
}
guard let image = NSImage(contentsOfFile: args[1]) else {
    FileHandle.standardError.write("cannot load \(args[1])\n".data(using: .utf8)!)
    exit(1)
}
guard let rep = NSBitmapImageRep(
    bitmapDataPlanes: nil, pixelsWide: size, pixelsHigh: size, bitsPerSample: 8,
    samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
    colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)
else { exit(1) }
rep.size = NSSize(width: size, height: size)
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
NSColor.clear.set()
NSRect(x: 0, y: 0, width: size, height: size).fill()
image.draw(in: NSRect(x: 0, y: 0, width: size, height: size),
           from: .zero, operation: .sourceOver, fraction: 1.0)
NSGraphicsContext.restoreGraphicsState()
if args[2].hasSuffix(".rgba") {
    // Non-premultiplied, 4 bytes per pixel; bytesPerRow may be padded.
    guard let data = rep.bitmapData else { exit(1) }
    var out = Data(capacity: size * size * 4)
    for y in 0..<size {
        out.append(data.advanced(by: y * rep.bytesPerRow), count: size * 4)
    }
    try! out.write(to: URL(fileURLWithPath: args[2]))
} else {
    guard let png = rep.representation(using: .png, properties: [:]) else { exit(1) }
    try! png.write(to: URL(fileURLWithPath: args[2]))
}
