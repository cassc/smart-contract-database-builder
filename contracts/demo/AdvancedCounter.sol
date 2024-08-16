// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import "./Counter.sol";

contract AdvancedCounter is Counter {
    function reset() public {
        count = privateFunction();
    }

    function privateFunction () private returns (uint) {
        return block.timestamp;
    }
}
