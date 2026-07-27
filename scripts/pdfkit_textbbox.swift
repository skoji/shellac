// pdfkit_textbbox: locate a text anchor on one page of a PDF using PDFKit,
// and report its bounds in PDFKit "page space" (the same space PDFAnnotation
// bounds/PDFPage box queries use) plus page geometry, as JSON.
//
// usage: pdfkit_textbbox <pdf> <page (1-based)>
//
// Output (always valid JSON, even on "not found" -- absence is reported via
// "found":false, not a non-zero exit, so callers can treat "no anchor text
// on this page" as a normal, expected outcome):
//   {
//     "found": bool,
//     "needle": string,       // "" if found == false
//     "bounds": {"llx":Double,"lly":Double,"urx":Double,"ury":Double},
//     "pageRotation": Int,    // PDFPage.rotation: 0/90/180/270
//     "mediaBox": {"llx":...,"lly":...,"urx":...,"ury":...},
//     "cropBox": {"llx":...,"lly":...,"urx":...,"ury":...}
//   }
//
// Needle selection (position-verification task): the first run of >= 3
// consecutive non-whitespace characters in the page's extracted text
// (PDFDocument.page(at:).string), matching the
// tokenization pdftotext produces for its own bbox output closely enough
// that harness/checks.go can cross-check the same needle independently via
// `pdftotext -bbox-layout`. If no run reaches 3 characters, falls back to
// the longest run on the page. If the page has no extractable text at all,
// "found" is false and every other field is zeroed.
//
// Exit codes: 0 on success (including the "found":false case -- that is
// not an error, just "no anchor available"). Non-zero only for usage
// errors or an unreadable PDF/page, matching the other pdfkit_* helpers.
import Foundation
import PDFKit

struct RectJSON: Codable {
    var llx: Double
    var lly: Double
    var urx: Double
    var ury: Double
}

struct AnchorResult: Codable {
    var found: Bool
    var needle: String
    var bounds: RectJSON
    var pageRotation: Int
    var mediaBox: RectJSON
    var cropBox: RectJSON
}

func emit(_ r: AnchorResult) {
    let encoder = JSONEncoder()
    if let data = try? encoder.encode(r) {
        FileHandle.standardOutput.write(data)
        FileHandle.standardOutput.write(Data("\n".utf8))
    }
}

func zeroRect() -> RectJSON { RectJSON(llx: 0, lly: 0, urx: 0, ury: 0) }

func rectJSON(from rect: CGRect) -> RectJSON {
    RectJSON(llx: Double(rect.minX), lly: Double(rect.minY), urx: Double(rect.maxX), ury: Double(rect.maxY))
}

// First run of >= 3 consecutive non-whitespace characters in `text`; if
// none reaches that length, the single longest run. Returns nil if `text`
// has no non-whitespace runs at all.
func chooseNeedle(in text: String) -> String? {
    let runs = text.components(separatedBy: .whitespacesAndNewlines).filter { !$0.isEmpty }
    if let firstLongEnough = runs.first(where: { $0.count >= 3 }) {
        return firstLongEnough
    }
    return runs.max(by: { $0.count < $1.count })
}

let args = CommandLine.arguments
guard args.count == 3, let pageNumberOneBased = Int(args[2]), pageNumberOneBased >= 1 else {
    FileHandle.standardError.write(Data("usage: pdfkit_textbbox <pdf> <page (1-based)>\n".utf8))
    exit(1)
}
let url = URL(fileURLWithPath: args[1])

guard let doc = PDFDocument(url: url) else {
    FileHandle.standardError.write(Data("cannot open \(args[1])\n".utf8))
    exit(2)
}
guard let page = doc.page(at: pageNumberOneBased - 1) else {
    FileHandle.standardError.write(Data("no page \(pageNumberOneBased) in \(args[1])\n".utf8))
    exit(2)
}

let mediaBox = rectJSON(from: page.bounds(for: .mediaBox))
let cropBox = rectJSON(from: page.bounds(for: .cropBox))
let rotation = page.rotation

guard let pageText = page.string, let needle = chooseNeedle(in: pageText) else {
    emit(AnchorResult(found: false, needle: "", bounds: zeroRect(), pageRotation: rotation, mediaBox: mediaBox, cropBox: cropBox))
    exit(0)
}

let matches = doc.findString(needle, withOptions: [])
guard let selection = matches.first(where: { $0.pages.contains(page) }) else {
    emit(AnchorResult(found: false, needle: needle, bounds: zeroRect(), pageRotation: rotation, mediaBox: mediaBox, cropBox: cropBox))
    exit(0)
}

let bounds = rectJSON(from: selection.bounds(for: page))
emit(AnchorResult(found: true, needle: needle, bounds: bounds, pageRotation: rotation, mediaBox: mediaBox, cropBox: cropBox))
