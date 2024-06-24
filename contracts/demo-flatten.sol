// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

library Math {
    function add(uint256 a, uint256 b) internal pure returns (uint256) {
        return a + b;
    }

    function subtract(uint256 a, uint256 b) internal pure returns (uint256) {
        require(b <= a, "Subtraction overflow");
        return a - b;
    }
}


interface ICounter {
    function increment() external;
    function decrement() external;
    function getCount() external view returns (uint256);
}


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



contract AdvancedCounter is Counter {
    function reset() public {
        count = 0;
    }
}
