// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

interface ICounter {
    function increment() external;
    function decrement() external;
    function getCount() external view returns (uint256);
}
