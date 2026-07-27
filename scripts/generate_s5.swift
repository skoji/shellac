// generate_s5: create sample S5 — a PDF that has gone through a PDFKit
// full rewrite (PDFDocument.write(to:)) after adding a seed highlight
// annotation on page 0.
// usage: generate_s5 <in.pdf> <out.pdf>
import Foundation
import PDFKit
import AppKit

let args = CommandLine.arguments
guard args.count == 3 else {
    FileHandle.standardError.write(Data("usage: generate_s5 <in.pdf> <out.pdf>\n".utf8))
    exit(1)
}
let inURL = URL(fileURLWithPath: args[1])
let outURL = URL(fileURLWithPath: args[2])

guard let doc = PDFDocument(url: inURL) else {
    FileHandle.standardError.write(Data("cannot open \(args[1])\n".utf8))
    exit(2)
}
guard let page = doc.page(at: 0) else {
    FileHandle.standardError.write(Data("no page 0 in \(args[1])\n".utf8))
    exit(2)
}
let annotation = PDFAnnotation(
    bounds: CGRect(x: 50, y: 50, width: 150, height: 20),
    forType: .highlight,
    withProperties: nil)
annotation.color = NSColor.yellow.withAlphaComponent(0.5)
_ = annotation.setValue("shellac-s5-seed", forAnnotationKey: .name)
// userName backs the annotation's /T (author) key. Left unset, PDFKit fills
// it in with the running account's real full name, which must not leak into
// a published fixture.
annotation.userName = "shellac-corpus"
page.addAnnotation(annotation)

guard doc.write(to: outURL) else {
    FileHandle.standardError.write(Data("write failed: \(args[2])\n".utf8))
    exit(3)
}
