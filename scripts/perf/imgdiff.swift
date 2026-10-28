import CoreGraphics
import Foundation
import ImageIO

func pixels(_ path: String) -> (Int, Int, [UInt8])? {
    guard let src = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil),
          let img = CGImageSourceCreateImageAtIndex(src, 0, nil) else { return nil }
    let w = img.width, h = img.height
    var buf = [UInt8](repeating: 0, count: w * h * 4)
    let ctx = CGContext(data: &buf, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                        space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    ctx.draw(img, in: CGRect(x: 0, y: 0, width: w, height: h))
    return (w, h, buf)
}

let args = CommandLine.arguments
guard args.count == 3, let a = pixels(args[1]), let b = pixels(args[2]) else {
    print("用法：imgdiff.swift <a.png> <b.png>")
    exit(2)
}
if a.0 != b.0 || a.1 != b.1 {
    print("尺寸不同：\(a.0)x\(a.1) vs \(b.0)x\(b.1)")
    exit(1)
}
var diff = 0, minX = Int.max, minY = Int.max, maxX = -1, maxY = -1
for y in 0..<a.1 {
    for x in 0..<a.0 {
        let i = (y * a.0 + x) * 4
        if a.2[i] != b.2[i] || a.2[i + 1] != b.2[i + 1] || a.2[i + 2] != b.2[i + 2] {
            diff += 1
            minX = min(minX, x); minY = min(minY, y); maxX = max(maxX, x); maxY = max(maxY, y)
        }
    }
}
if diff == 0 {
    print("相同")
} else {
    print("不同像素 \(diff)，范围 x \(minX)-\(maxX) y \(minY)-\(maxY)")
}
