// Minimal sample program for the oxide-sloc CMake example.
// It exists only so the `sloc` target has real source lines to count.

#include <iostream>
#include <string>

namespace {

// Return a friendly greeting for the given name.
std::string greeting(const std::string& name) {
    if (name.empty()) {
        return "Hello, world!";
    }
    return "Hello, " + name + "!";
}

// Sum the integers in the half-open range [begin, end).
long long sum_range(int begin, int end) {
    long long total = 0;
    for (int i = begin; i < end; ++i) {
        total += i;
    }
    return total;
}

}  // namespace

int main(int argc, char** argv) {
    const std::string name = (argc > 1) ? argv[1] : "";
    std::cout << greeting(name) << '\n';
    std::cout << "sum(0..10) = " << sum_range(0, 10) << '\n';
    return 0;
}
