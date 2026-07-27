// pdfkit_text: print PDFDocument.string of a PDF to stdout.
// usage: pdfkit_text <pdf>
// exit 2 if the document cannot be opened.
import Foundation
import PDFKit

let args = CommandLine.arguments
guard args.count == 2 else {
    FileHandle.standardError.write(Data("usage: pdfkit_text <pdf>\n".utf8))
    exit(1)
}
let url = URL(fileURLWithPath: args[1])
guard let doc = PDFDocument(url: url) else {
    FileHandle.standardError.write(Data("cannot open \(args[1])\n".utf8))
    exit(2)
}
let text = doc.string ?? ""
FileHandle.standardOutput.write(Data(text.utf8))
