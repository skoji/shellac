// pdfkit_check: reload a PDF with PDFKit and report facts as JSON.
// usage: pdfkit_check <pdf> <expectedPageCount> <comma-separated-expected-NM or "-"> [<needle or -> <anchorNM or ->]
// Output: {"loaded":bool,"pageCount":int,"foundNM":[...],"annotTypes":[...],"thumbnailOK":bool,
//          "c11bChecked":bool,"c11bPass":bool,"c11bDetail":string}
// foundNM collects value(forAnnotationKey: .name) over all pages; if an
// expected-NM list is given, foundNM is filtered to those ids.
// The expectedPageCount argument is informational (judgement happens in the
// Go harness); it is accepted for interface stability.
//
// C11b (position verification): when both <needle>
// and <anchorNM> are given (neither is "-"), independently re-locates
// needle on the page that holds the annotation named anchorNM (via
// PDFDocument.findString, the same API family pdfkit_textbbox used to
// place it -- this is the "dependent" (PDFKit-only) half of the position
// check; the independent half is C11a in the Go harness, via pdftotext).
// Passes when that annotation's .bounds is within 3pt of the freshly
// re-located needle's bounds(for:) on all four sides. If anchorNM's
// annotation is missing, or needle can't be re-located, c11bChecked is
// still true but c11bPass is false with a detail explaining why (this is a
// hard check, unlike C11a's "skip if no anchor" -- by the time this is
// called the caller has already confirmed an anchor exists and the
// annotation was supposed to land there).
import Foundation
import PDFKit

struct CheckResult: Codable {
    var loaded: Bool
    var pageCount: Int
    var foundNM: [String]
    var annotTypes: [String]
    var thumbnailOK: Bool
    var c11bChecked: Bool
    var c11bPass: Bool
    var c11bDetail: String
}

func emit(_ r: CheckResult) {
    let encoder = JSONEncoder()
    if let data = try? encoder.encode(r) {
        FileHandle.standardOutput.write(data)
        FileHandle.standardOutput.write(Data("\n".utf8))
    }
}

let args = CommandLine.arguments
guard args.count == 4 || args.count == 6 else {
    FileHandle.standardError.write(Data("usage: pdfkit_check <pdf> <expectedPageCount> <nm,nm,... or -> [<needle or -> <anchorNM or ->]\n".utf8))
    exit(1)
}
let url = URL(fileURLWithPath: args[1])
let expectedNM: [String]? = args[3] == "-" ? nil : args[3].split(separator: ",").map(String.init)
let needleArg: String? = (args.count == 6 && args[4] != "-") ? args[4] : nil
let anchorNMArg: String? = (args.count == 6 && args[5] != "-") ? args[5] : nil

var result = CheckResult(loaded: false, pageCount: 0, foundNM: [], annotTypes: [], thumbnailOK: false,
                          c11bChecked: false, c11bPass: false, c11bDetail: "")

guard let doc = PDFDocument(url: url) else {
    emit(result)
    exit(0)
}
result.loaded = true
result.pageCount = doc.pageCount

var names: [String] = []
var types: [String] = []
var thumbOK = true
var annotationsByName: [String: PDFAnnotation] = [:]
var pageByAnnotationName: [String: PDFPage] = [:]
for i in 0..<doc.pageCount {
    guard let page = doc.page(at: i) else {
        thumbOK = false
        continue
    }
    for annotation in page.annotations {
        if let name = annotation.value(forAnnotationKey: .name) as? String {
            names.append(name)
            annotationsByName[name] = annotation
            pageByAnnotationName[name] = page
        }
        types.append(annotation.type ?? "?")
    }
    let thumb = page.thumbnail(of: CGSize(width: 120, height: 160), for: .mediaBox)
    if thumb.size.width <= 0 || thumb.size.height <= 0 {
        thumbOK = false
    }
}
if let expected = expectedNM {
    let expectedSet = Set(expected)
    names = names.filter { expectedSet.contains($0) }
}
result.foundNM = names
result.annotTypes = types
result.thumbnailOK = thumbOK

if let needle = needleArg, let anchorNM = anchorNMArg {
    result.c11bChecked = true
    if let annotation = annotationsByName[anchorNM], let page = pageByAnnotationName[anchorNM] {
        let matches = doc.findString(needle, withOptions: [])
        if let selection = matches.first(where: { $0.pages.contains(page) }) {
            let needleBounds = selection.bounds(for: page)
            let annotBounds = annotation.bounds
            let tol: CGFloat = 3.0
            let dMinX = abs(annotBounds.minX - needleBounds.minX)
            let dMinY = abs(annotBounds.minY - needleBounds.minY)
            let dMaxX = abs(annotBounds.maxX - needleBounds.maxX)
            let dMaxY = abs(annotBounds.maxY - needleBounds.maxY)
            result.c11bPass = dMinX <= tol && dMinY <= tol && dMaxX <= tol && dMaxY <= tol
            result.c11bDetail = "annotation.bounds=\(annotBounds) needle.bounds=\(needleBounds) maxDelta=\(max(dMinX, dMinY, dMaxX, dMaxY))"
        } else {
            result.c11bPass = false
            result.c11bDetail = "needle \(needle) not re-locatable via PDFDocument.findString on the annotation's page"
        }
    } else {
        result.c11bPass = false
        result.c11bDetail = "annotation named \(anchorNM) not found"
    }
}

emit(result)
