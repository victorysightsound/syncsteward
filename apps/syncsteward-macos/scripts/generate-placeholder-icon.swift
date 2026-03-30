#!/usr/bin/env swift

import AppKit
import Foundation

struct IconSlot {
    let filename: String
    let pixels: Int
}

let slots: [IconSlot] = [
    .init(filename: "icon_16x16.png", pixels: 16),
    .init(filename: "icon_16x16@2x.png", pixels: 32),
    .init(filename: "icon_32x32.png", pixels: 32),
    .init(filename: "icon_32x32@2x.png", pixels: 64),
    .init(filename: "icon_128x128.png", pixels: 128),
    .init(filename: "icon_128x128@2x.png", pixels: 256),
    .init(filename: "icon_256x256.png", pixels: 256),
    .init(filename: "icon_256x256@2x.png", pixels: 512),
    .init(filename: "icon_512x512.png", pixels: 512),
    .init(filename: "icon_512x512@2x.png", pixels: 1024),
]

let masterPixels = 1024
let backgroundColor = NSColor(calibratedRed: 0.24, green: 0.53, blue: 0.88, alpha: 1)
let symbolName = "arrow.triangle.2.circlepath"

guard CommandLine.arguments.count == 2 else {
    fputs("usage: generate-placeholder-icon.swift <iconset-dir>\n", stderr)
    exit(1)
}

let outputDir = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
try FileManager.default.createDirectory(at: outputDir, withIntermediateDirectories: true)

func renderMasterPNG(to destination: URL) throws {
    let size = CGFloat(masterPixels)
    guard let bitmap = NSBitmapImageRep(
        bitmapDataPlanes: nil,
        pixelsWide: masterPixels,
        pixelsHigh: masterPixels,
        bitsPerSample: 8,
        samplesPerPixel: 4,
        hasAlpha: true,
        isPlanar: false,
        colorSpaceName: .deviceRGB,
        bytesPerRow: 0,
        bitsPerPixel: 0
    ) else {
        throw NSError(domain: "SyncStewardIcon", code: 4, userInfo: [NSLocalizedDescriptionKey: "Unable to allocate bitmap"])
    }
    bitmap.size = NSSize(width: size, height: size)

    guard let context = NSGraphicsContext(bitmapImageRep: bitmap) else {
        throw NSError(domain: "SyncStewardIcon", code: 5, userInfo: [NSLocalizedDescriptionKey: "Unable to create graphics context"])
    }

    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = context
    NSGraphicsContext.current?.imageInterpolation = .high

    let backgroundRect = NSRect(x: 0, y: 0, width: size, height: size)
    let background = NSBezierPath(roundedRect: backgroundRect, xRadius: size * 0.22, yRadius: size * 0.22)
    backgroundColor.setFill()
    background.fill()

    guard let baseSymbol = NSImage(systemSymbolName: symbolName, accessibilityDescription: nil) else {
        throw NSError(domain: "SyncStewardIcon", code: 1, userInfo: [NSLocalizedDescriptionKey: "Unable to load SF Symbol \(symbolName)"])
    }

    var symbolConfig = NSImage.SymbolConfiguration(pointSize: size * 0.56, weight: .regular)
    if #available(macOS 12.0, *) {
        symbolConfig = symbolConfig.applying(NSImage.SymbolConfiguration(paletteColors: [.white]))
    }
    let symbol = baseSymbol.withSymbolConfiguration(symbolConfig) ?? baseSymbol

    let symbolSize = size * 0.56
    let symbolRect = NSRect(
        x: (size - symbolSize) / 2,
        y: (size - symbolSize) / 2,
        width: symbolSize,
        height: symbolSize
    )
    symbol.draw(in: symbolRect)
    context.flushGraphics()
    NSGraphicsContext.restoreGraphicsState()

    guard let png = bitmap.representation(using: .png, properties: [:]) else {
        throw NSError(domain: "SyncStewardIcon", code: 2, userInfo: [NSLocalizedDescriptionKey: "Unable to encode rendered icon"])
    }
    try png.write(to: destination)
}

func runSipsResize(input: URL, output: URL, pixels: Int) throws {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/sips")
    process.arguments = ["-z", String(pixels), String(pixels), input.path, "--out", output.path]
    try process.run()
    process.waitUntilExit()
    guard process.terminationStatus == 0 else {
        throw NSError(domain: "SyncStewardIcon", code: 3, userInfo: [NSLocalizedDescriptionKey: "sips resize failed for \(output.lastPathComponent)"])
    }
}

let masterURL = outputDir.appendingPathComponent("placeholder-master-1024.png")
try renderMasterPNG(to: masterURL)

for slot in slots {
    let destination = outputDir.appendingPathComponent(slot.filename)
    if slot.pixels == masterPixels {
        if FileManager.default.fileExists(atPath: destination.path) {
            try FileManager.default.removeItem(at: destination)
        }
        try FileManager.default.copyItem(at: masterURL, to: destination)
    } else {
        try runSipsResize(input: masterURL, output: destination, pixels: slot.pixels)
    }
    print(destination.path)
}

try FileManager.default.removeItem(at: masterURL)
