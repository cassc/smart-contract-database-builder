// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import "./Math.sol";
import "./ICounter.sol";

contract Counter is ICounter {
    using Math for uint256;

    uint256 public count;

    function increment() public override {
        count = count.add(1);
    }

    function decrement() public override {
        count = count.subtract(1);
    }

    function getCount() public view override returns (uint256) {
        return count;
    }
}
