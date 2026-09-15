/* C++ class and namespace test */
#include <string>
#include <vector>

namespace myapp {

class Base {
public:
    virtual ~Base() = default;
    virtual int compute() = 0;
    void common();

protected:
    int base_value;
};

class Derived : public Base {
public:
    Derived(int val);
    ~Derived() override;

    int compute() override;
    void extra();

private:
    int derived_value;
    std::string name;
};

// Implementation
Derived::Derived(int val) : base_value(val), derived_value(val * 2) {}

Derived::~Derived() {}

int Derived::compute() {
    return base_value + derived_value;
}

void Derived::extra() {
    name = "derived";
}

void Base::common() {
    base_value = 0;
}

// Namespace-level function
int namespace_func(int x) {
    Derived d(x);
    return d.compute();
}

} // namespace myapp

/* Template test */
template <typename T>
T max_val(T a, T b) {
    return a > b ? a : b;
}

/* Lambda */
void lambda_test() {
    auto add = [](int a, int b) { return a + b; };
    int result = add(1, 2);
}
