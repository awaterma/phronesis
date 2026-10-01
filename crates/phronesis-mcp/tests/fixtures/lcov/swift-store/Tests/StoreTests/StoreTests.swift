import XCTest
@testable import Store

final class StoreTests: XCTestCase {
    func testLoad() {
        XCTAssertEqual(Store().load("x"), "x")
    }
}