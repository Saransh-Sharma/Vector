import ApplicationServices
import AppKit
import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

struct Request: Decodable {
    let protocolVersion: String
    let id: String
    let grant: String
    let action: String
    let params: [String: JSONValue]?
}

struct Response: Encodable {
    let protocolVersion = "1.0"
    let id: String
    let ok: Bool
    let result: JSONValue?
    let error: HelperError?
}

struct HelperError: Encodable {
    let code: String
    let message: String
}

enum JSONValue: Codable {
    case string(String), number(Double), bool(Bool), object([String: JSONValue]), array([JSONValue]), null

    init(from decoder: Decoder) throws {
        let value = try decoder.singleValueContainer()
        if value.decodeNil() { self = .null }
        else if let decoded = try? value.decode(Bool.self) { self = .bool(decoded) }
        else if let decoded = try? value.decode(Double.self) { self = .number(decoded) }
        else if let decoded = try? value.decode(String.self) { self = .string(decoded) }
        else if let decoded = try? value.decode([String: JSONValue].self) { self = .object(decoded) }
        else { self = .array(try value.decode([JSONValue].self)) }
    }

    func encode(to encoder: Encoder) throws {
        var value = encoder.singleValueContainer()
        switch self {
        case .string(let item): try value.encode(item)
        case .number(let item): try value.encode(item)
        case .bool(let item): try value.encode(item)
        case .object(let item): try value.encode(item)
        case .array(let item): try value.encode(item)
        case .null: try value.encodeNil()
        }
    }

    var string: String? { if case .string(let value) = self { value } else { nil } }
    var number: Double? { if case .number(let value) = self { value } else { nil } }
}

let encoder = JSONEncoder()
encoder.outputFormatting = [.sortedKeys]
let decoder = JSONDecoder()
let expectedGrant = ProcessInfo.processInfo.environment["VECTOR_COMPUTER_GRANT"]
let allowedRunRoot = ProcessInfo.processInfo.environment["VECTOR_RUN_DIR"].map { URL(fileURLWithPath: $0).standardizedFileURL }

while let line = readLine() {
    do {
        let request = try decoder.decode(Request.self, from: Data(line.utf8))
        let response = try handle(request)
        print(String(decoding: try encoder.encode(response), as: UTF8.self))
    } catch {
        let response = Response(id: "unknown", ok: false, result: nil, error: HelperError(code: "VCTR_CONFIG_INVALID", message: error.localizedDescription))
        print(String(decoding: try! encoder.encode(response), as: UTF8.self))
    }
    fflush(stdout)
}

func handle(_ request: Request) throws -> Response {
    guard request.protocolVersion == "1.0" else { return failure(request, "VCTR_CONFIG_INVALID", "Unsupported computer protocol") }
    guard let expectedGrant, request.grant == expectedGrant else { return failure(request, "VCTR_POLICY_DENIED", "The run-scoped computer grant is missing or invalid") }

    switch request.action {
    case "status":
        return success(request, .object([
            "accessibility": .bool(AXIsProcessTrusted()),
            "screenCapture": .bool(CGPreflightScreenCaptureAccess()),
            "platform": .string("macOS")
        ]))
    case "request-permissions":
        let options = ["AXTrustedCheckOptionPrompt": true] as CFDictionary
        _ = AXIsProcessTrustedWithOptions(options)
        _ = CGRequestScreenCaptureAccess()
        return success(request, .object(["requested": .bool(true)]))
    case "inspect-windows":
        guard CGPreflightScreenCaptureAccess() else { return failure(request, "VCTR_POLICY_DENIED", "Screen Recording permission is required") }
        let raw = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
        let windows = raw.prefix(200).map { window -> JSONValue in
            .object([
                "id": .number((window[kCGWindowNumber as String] as? NSNumber)?.doubleValue ?? 0),
                "owner": .string(window[kCGWindowOwnerName as String] as? String ?? "Unknown"),
                "title": .string(window[kCGWindowName as String] as? String ?? ""),
                "layer": .number((window[kCGWindowLayer as String] as? NSNumber)?.doubleValue ?? 0)
            ])
        }
        return success(request, .array(windows))
    case "screenshot":
        guard CGPreflightScreenCaptureAccess() else { return failure(request, "VCTR_POLICY_DENIED", "Screen Recording permission is required") }
        guard let path = request.params?["path"]?.string else { return failure(request, "VCTR_CONFIG_INVALID", "screenshot requires params.path") }
        let target = URL(fileURLWithPath: path).standardizedFileURL
        guard let allowedRunRoot, target.path.hasPrefix(allowedRunRoot.path + "/") else { return failure(request, "VCTR_POLICY_DENIED", "Screenshot path must be inside VECTOR_RUN_DIR") }
        guard let image = CGDisplayCreateImage(CGMainDisplayID()) else { return failure(request, "VCTR_RUN_FAILED", "The main display could not be captured") }
        try FileManager.default.createDirectory(at: target.deletingLastPathComponent(), withIntermediateDirectories: true)
        guard let destination = CGImageDestinationCreateWithURL(target as CFURL, UTType.png.identifier as CFString, 1, nil) else { return failure(request, "VCTR_RUN_FAILED", "The screenshot destination could not be opened") }
        CGImageDestinationAddImage(destination, image, nil)
        guard CGImageDestinationFinalize(destination) else { return failure(request, "VCTR_RUN_FAILED", "The screenshot could not be written") }
        return success(request, .object(["path": .string(target.path), "width": .number(Double(image.width)), "height": .number(Double(image.height))]))
    case "click":
        guard trusted(request) else { return permissionFailure(request) }
        guard let x = request.params?["x"]?.number, let y = request.params?["y"]?.number else { return failure(request, "VCTR_CONFIG_INVALID", "click requires numeric x and y") }
        let point = CGPoint(x: x, y: y)
        CGEvent(mouseEventSource: nil, mouseType: .leftMouseDown, mouseCursorPosition: point, mouseButton: .left)?.post(tap: .cghidEventTap)
        CGEvent(mouseEventSource: nil, mouseType: .leftMouseUp, mouseCursorPosition: point, mouseButton: .left)?.post(tap: .cghidEventTap)
        return success(request, .object(["x": .number(x), "y": .number(y)]))
    case "type":
        guard trusted(request) else { return permissionFailure(request) }
        guard let text = request.params?["text"]?.string else { return failure(request, "VCTR_CONFIG_INVALID", "type requires text") }
        let utf16 = Array(text.utf16)
        let event = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: true)
        event?.keyboardSetUnicodeString(stringLength: utf16.count, unicodeString: utf16)
        event?.post(tap: .cghidEventTap)
        CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: false)?.post(tap: .cghidEventTap)
        return success(request, .object(["characters": .number(Double(text.count))]))
    case "key":
        guard trusted(request) else { return permissionFailure(request) }
        guard let code = request.params?["code"]?.number else { return failure(request, "VCTR_CONFIG_INVALID", "key requires a numeric macOS virtual key code") }
        CGEvent(keyboardEventSource: nil, virtualKey: CGKeyCode(code), keyDown: true)?.post(tap: .cghidEventTap)
        CGEvent(keyboardEventSource: nil, virtualKey: CGKeyCode(code), keyDown: false)?.post(tap: .cghidEventTap)
        return success(request, .object(["code": .number(code)]))
    case "scroll":
        guard trusted(request) else { return permissionFailure(request) }
        let dy = Int32(request.params?["dy"]?.number ?? 0)
        CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: dy, wheel2: 0, wheel3: 0)?.post(tap: .cghidEventTap)
        return success(request, .object(["dy": .number(Double(dy))]))
    case "wait":
        let milliseconds = min(max(request.params?["milliseconds"]?.number ?? 250, 0), 30_000)
        Thread.sleep(forTimeInterval: milliseconds / 1_000)
        return success(request, .object(["milliseconds": .number(milliseconds)]))
    default:
        return failure(request, "VCTR_CAPABILITY_UNSATISFIED", "This computer action is not implemented by the macOS helper")
    }
}

func trusted(_ request: Request) -> Bool { AXIsProcessTrusted() }
func permissionFailure(_ request: Request) -> Response { failure(request, "VCTR_POLICY_DENIED", "Accessibility permission is required") }
func success(_ request: Request, _ result: JSONValue) -> Response { Response(id: request.id, ok: true, result: result, error: nil) }
func failure(_ request: Request, _ code: String, _ message: String) -> Response { Response(id: request.id, ok: false, result: nil, error: HelperError(code: code, message: message)) }
